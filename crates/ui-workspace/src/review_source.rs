//! What the Duplicates and AI Review views need from the engine ([`ReviewSource`]), and its implementation
//! over the catalog ([`CatalogReview`], on top of `ps_service::{groups, review, insights, ai}`).
//!
//! Every method blocks and is only called from background tasks, never on the UI thread (ADR-0005).

use std::sync::{Arc, Mutex};

use ps_model::FileId;
use ps_model::settings::Settings;
use ps_service::ai::AiStatus;
use ps_service::groups::SeparateOutcome;
use ps_service::insights::{Correction, InsightFilter, InsightItem, InsightStamp, InsightStore};
use ps_service::review::{
    ReviewDetail, ReviewFilter, ReviewList, ReviewSort, ReviewStore, SuggestionCard,
};
use ps_service::secrets::Secrets;

use crate::library_backend::CatalogBackend;

/// Duplicate review and AI review data.
pub trait ReviewSource: Send + Sync + 'static {
    fn groups(&self, filter: ReviewFilter, sort: ReviewSort) -> anyhow::Result<ReviewList>;
    fn group(&self, id: FileId) -> anyhow::Result<ReviewDetail>;
    /// Make `file` the keeper ("chosen by you"); returns the group as it is now.
    fn set_keeper(&self, group: FileId, file: FileId) -> anyhow::Result<ReviewDetail>;
    /// "This is a different photo" (ADR-0033).
    fn separate(&self, group: FileId, file: FileId) -> anyhow::Result<SeparateOutcome>;
    fn merge(&self, a: FileId, b: FileId) -> anyhow::Result<ReviewDetail>;
    /// Undo the user merges inside `group` (back to automatic matching).
    fn undo_merges(&self, group: FileId) -> anyhow::Result<ReviewDetail>;
    fn set_reviewed(&self, group: FileId, reviewed: bool) -> anyhow::Result<bool>;
    fn suggestions(&self, limit: u64) -> anyhow::Result<Vec<SuggestionCard>>;
    fn dismiss(&self, a: FileId, b: FileId) -> anyhow::Result<()>;
    fn explain(&self, a: FileId, b: FileId) -> anyhow::Result<Vec<String>>;

    fn ai_stamp(&self) -> anyhow::Result<InsightStamp>;
    fn ai_items(&self, filter: &InsightFilter, limit: u64) -> anyhow::Result<Vec<InsightItem>>;
    fn ai_correct(&self, content: &str, c: Correction) -> anyhow::Result<()>;
    fn ai_clear(&self, content: &str) -> anyhow::Result<()>;
    /// Accept or reject an item's advisory suggestion; returns the files excluded.
    fn ai_decide(&self, content: &str, accept: bool) -> anyhow::Result<usize>;
    /// Undo a decision: the suggestion is open again and its exclusions are removed; returns the files
    /// included again.
    fn ai_undo_decision(&self, _content: &str) -> anyhow::Result<usize> {
        Ok(0)
    }
    fn ai_status(&self) -> anyhow::Result<AiStatus>;
    /// Start (or resume) the queue for the configured provider; refuses a hosted one without consent.
    fn ai_start(&self, settings: &Settings, secrets: &Secrets) -> anyhow::Result<u64>;
    fn ai_pause(&self) -> anyhow::Result<()>;
    /// Queue the results of older prompts again (explicit; sends nothing until the queue starts).
    fn ai_redescribe_older(&self) -> anyhow::Result<u64> {
        Ok(0)
    }
    /// The provider, server address or consent changed (ADR-0047): pause a running queue so nothing more is
    /// sent to the old destination; the user starts it again for the new one. `true` when a queue was paused.
    fn ai_destination_changed(&self) -> anyhow::Result<bool> {
        Ok(false)
    }
}

/// [`ReviewSource`] over the catalog behind a [`CatalogBackend`].
pub struct CatalogReview {
    backend: Arc<CatalogBackend>,
    review: std::sync::OnceLock<ReviewStore>,
    ai: Mutex<Option<(String, ps_service::ai::AiService)>>,
}

impl CatalogReview {
    pub fn new(backend: Arc<CatalogBackend>) -> Self {
        Self {
            backend,
            review: std::sync::OnceLock::new(),
            ai: Mutex::new(None),
        }
    }

    fn store(&self) -> anyhow::Result<&ReviewStore> {
        let scan = self.backend.scan()?;
        Ok(self.review.get_or_init(|| ReviewStore::for_service(scan)))
    }

    /// The library's store, so corrections made here are what the gallery's inspector and filters see.
    fn istore(&self) -> anyhow::Result<&InsightStore> {
        self.backend.insight_store()
    }
}

impl ReviewSource for CatalogReview {
    fn groups(&self, filter: ReviewFilter, sort: ReviewSort) -> anyhow::Result<ReviewList> {
        Ok(ps_service::review::list(
            self.backend.scan()?,
            self.store()?,
            filter,
            sort,
        )?)
    }
    fn group(&self, id: FileId) -> anyhow::Result<ReviewDetail> {
        Ok(ps_service::review::show(
            self.backend.scan()?,
            self.store()?,
            id,
        )?)
    }
    fn set_keeper(&self, group: FileId, file: FileId) -> anyhow::Result<ReviewDetail> {
        let scan = self.backend.scan()?;
        let d = scan.groups().set_keeper(group, file)?;
        Ok(ps_service::review::show(scan, self.store()?, d.id)?)
    }
    fn separate(&self, group: FileId, file: FileId) -> anyhow::Result<SeparateOutcome> {
        Ok(self.backend.scan()?.groups().separate(group, file)?)
    }
    fn merge(&self, a: FileId, b: FileId) -> anyhow::Result<ReviewDetail> {
        let scan = self.backend.scan()?;
        let d = scan.groups().merge(a, b)?;
        Ok(ps_service::review::show(scan, self.store()?, d.id)?)
    }
    fn undo_merges(&self, group: FileId) -> anyhow::Result<ReviewDetail> {
        let scan = self.backend.scan()?;
        let d = scan.groups().undo_merges(group)?;
        Ok(ps_service::review::show(scan, self.store()?, d.id)?)
    }
    fn set_reviewed(&self, group: FileId, reviewed: bool) -> anyhow::Result<bool> {
        Ok(ps_service::review::set_reviewed(
            self.backend.scan()?,
            self.store()?,
            group,
            reviewed,
        )?)
    }
    fn suggestions(&self, limit: u64) -> anyhow::Result<Vec<SuggestionCard>> {
        Ok(ps_service::review::suggestions(
            self.backend.scan()?,
            self.store()?,
            limit,
        )?)
    }
    fn dismiss(&self, a: FileId, b: FileId) -> anyhow::Result<()> {
        Ok(ps_service::review::dismiss(
            self.backend.scan()?,
            self.store()?,
            a,
            b,
        )?)
    }
    fn explain(&self, a: FileId, b: FileId) -> anyhow::Result<Vec<String>> {
        Ok(ps_service::review::explain(self.backend.scan()?, a, b)?)
    }

    fn ai_stamp(&self) -> anyhow::Result<InsightStamp> {
        Ok(ps_service::insights::stamp(
            self.backend.catalog()?,
            self.istore()?,
        )?)
    }
    fn ai_items(&self, filter: &InsightFilter, limit: u64) -> anyhow::Result<Vec<InsightItem>> {
        Ok(ps_service::insights::items(
            self.backend.catalog()?,
            self.istore()?,
            filter,
            limit,
        )?)
    }
    fn ai_correct(&self, content: &str, c: Correction) -> anyhow::Result<()> {
        Ok(ps_service::insights::correct(
            self.backend.catalog()?,
            self.istore()?,
            content,
            c,
        )?)
    }
    fn ai_clear(&self, content: &str) -> anyhow::Result<()> {
        Ok(ps_service::insights::clear(
            self.backend.catalog()?,
            self.istore()?,
            content,
        )?)
    }
    fn ai_decide(&self, content: &str, accept: bool) -> anyhow::Result<usize> {
        Ok(ps_service::insights::decide(
            self.backend.catalog()?,
            self.istore()?,
            content,
            accept,
        )?)
    }
    fn ai_undo_decision(&self, content: &str) -> anyhow::Result<usize> {
        Ok(ps_service::insights::undo(
            self.backend.catalog()?,
            self.istore()?,
            content,
        )?)
    }
    fn ai_status(&self) -> anyhow::Result<AiStatus> {
        if let Some((_, svc)) = self.ai.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
            return Ok(svc.status()?);
        }
        Ok(ps_service::ai::status(self.backend.catalog()?)?)
    }
    fn ai_start(&self, settings: &Settings, secrets: &Secrets) -> anyhow::Result<u64> {
        let (cfg, _) = ps_service::ai::provider_config(settings, secrets);
        ps_service::ai::check_consent(settings, &cfg).map_err(|e| anyhow::anyhow!("{e}"))?;
        let scan = self.backend.scan()?;
        let fingerprint = format!(
            "{:?}|{}|{}",
            settings.ai.provider, settings.ai.base_url, settings.ai.model
        );
        let mut ai = self.ai.lock().unwrap_or_else(|p| p.into_inner());
        if ai.as_ref().is_none_or(|(f, _)| *f != fingerprint) {
            if let Some((_, old)) = ai.take() {
                let _ = old.pause();
            }
            *ai = Some((
                fingerprint,
                ps_service::ai::AiService::from_settings(scan, settings, secrets)?,
            ));
        }
        let Some((_, svc)) = ai.as_ref() else {
            anyhow::bail!("the AI queue could not be created");
        };
        Ok(svc.start(false)?)
    }
    fn ai_redescribe_older(&self) -> anyhow::Result<u64> {
        Ok(ps_service::ai::redescribe_older_prompts(
            self.backend.catalog()?,
        )?)
    }
    fn ai_destination_changed(&self) -> anyhow::Result<bool> {
        // Drop the queue built for the old destination; the next start builds one for the new settings.
        let old = self.ai.lock().unwrap_or_else(|p| p.into_inner()).take();
        match old {
            Some((_, svc)) => {
                let running = svc.state() == ps_service::ai::QueueState::Running;
                svc.pause()?;
                log::info!("AI provider, address or consent changed: the AI queue is paused");
                Ok(running)
            }
            None => Ok(false),
        }
    }
    fn ai_pause(&self) -> anyhow::Result<()> {
        match self.ai.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
            Some((_, svc)) => Ok(svc.pause()?),
            None => Ok(ps_service::ai::mark_paused(self.backend.catalog()?)?),
        }
    }
}

/// The review source the views use (absent: views show an empty state).
pub struct ReviewGlobal(pub Option<Arc<dyn ReviewSource>>);

impl gpui_kit::Global for ReviewGlobal {}

impl ReviewGlobal {
    pub fn get(cx: &gpui_kit::App) -> Option<Arc<dyn ReviewSource>> {
        cx.try_global::<ReviewGlobal>().and_then(|g| g.0.clone())
    }
}

/// API-key access for the settings rows and the queue (tests install an in-memory keychain).
pub struct SecretsGlobal(pub Secrets);

impl gpui_kit::Global for SecretsGlobal {}

impl SecretsGlobal {
    pub fn get(cx: &gpui_kit::App) -> Secrets {
        cx.try_global::<SecretsGlobal>()
            .map(|s| s.0.clone())
            .unwrap_or_else(Secrets::os)
    }
}

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
    GroupExclusion, ReviewDetail, ReviewFilter, ReviewList, ReviewProgress, ReviewSort,
    ReviewStore, SuggestionCard,
};
use ps_service::secrets::Secrets;

use crate::library_backend::CatalogBackend;

/// Duplicate review and AI review data.
pub trait ReviewSource: Send + Sync + 'static {
    fn groups(&self, filter: ReviewFilter, sort: ReviewSort) -> anyhow::Result<ReviewList>;
    /// How many groups there are and how many are reviewed (the navigator's count).
    fn progress(&self) -> anyhow::Result<ReviewProgress> {
        Ok(ReviewProgress::from(&self.groups(
            ReviewFilter::default(),
            ReviewSort::Reclaimable,
        )?))
    }
    fn group(&self, id: FileId) -> anyhow::Result<ReviewDetail>;
    /// Make `file` the keeper ("chosen by you"); returns the group as it is now.
    fn set_keeper(&self, group: FileId, file: FileId) -> anyhow::Result<ReviewDetail>;
    /// "This is a different photo" (ADR-0033).
    fn separate(&self, group: FileId, file: FileId) -> anyhow::Result<SeparateOutcome>;
    fn merge(&self, a: FileId, b: FileId) -> anyhow::Result<ReviewDetail>;
    /// Undo the user merges inside `group` (back to automatic matching).
    fn undo_merges(&self, group: FileId) -> anyhow::Result<ReviewDetail>;
    fn set_reviewed(&self, group: FileId, reviewed: bool) -> anyhow::Result<bool>;
    /// Exclude every copy of `group` ("keep none") and mark it reviewed; returns what Undo needs.
    fn exclude_group(
        &self,
        _group: FileId,
        _reason: Option<String>,
    ) -> anyhow::Result<GroupExclusion> {
        anyhow::bail!("excluding a group is not available here")
    }
    /// Put an excluded group back: exactly as before with `undo`, else by clearing the user's exclusions.
    fn include_group(
        &self,
        _group: FileId,
        _undo: Option<GroupExclusion>,
    ) -> anyhow::Result<ReviewDetail> {
        anyhow::bail!("including a group is not available here")
    }
    /// The stacks of similar shots under `t` (best-of-stacks, ADR-0135).
    fn stacks(
        &self,
        _t: ps_model::StackThresholds,
    ) -> anyhow::Result<std::sync::Arc<ps_service::stacks::StacksView>> {
        anyhow::bail!("stacks are not available here")
    }
    /// Make `file` its stack's pick under `t`.
    fn stack_pick(&self, _t: ps_model::StackThresholds, _file: FileId) -> anyhow::Result<()> {
        anyhow::bail!("stacks are not available here")
    }
    /// Take `file` out of its stack under `t`.
    fn stack_take_out(&self, _t: ps_model::StackThresholds, _file: FileId) -> anyhow::Result<()> {
        anyhow::bail!("stacks are not available here")
    }
    /// Keep none of stack `index` under `t`: every copy of every shot excluded; returns what Undo needs.
    fn stack_exclude(
        &self,
        _t: ps_model::StackThresholds,
        _index: usize,
    ) -> anyhow::Result<ps_service::stacks::StackExclusion> {
        anyhow::bail!("stacks are not available here")
    }
    /// Undo [`Self::stack_exclude`].
    fn stack_undo_exclude(&self, _x: ps_service::stacks::StackExclusion) -> anyhow::Result<()> {
        anyhow::bail!("stacks are not available here")
    }
    /// Put `file` back into the stacks it was taken out of.
    fn stack_put_back(&self, _file: FileId) -> anyhow::Result<()> {
        anyhow::bail!("stacks are not available here")
    }
    /// Of `anchors`, the stacks marked reviewed.
    fn stacks_reviewed(
        &self,
        _anchors: Vec<ps_model::ContentId>,
    ) -> anyhow::Result<std::collections::HashSet<ps_model::ContentId>> {
        Ok(std::collections::HashSet::new())
    }
    /// Mark a stack (by its anchor) reviewed or not.
    fn set_stack_reviewed(
        &self,
        _anchor: ps_model::ContentId,
        _reviewed: bool,
    ) -> anyhow::Result<()> {
        anyhow::bail!("stacks are not available here")
    }
    /// A cleanup run is moving files: keeper, group and exclusion edits are refused until it ends (ADR-0099).
    /// Never blocks (called while rendering).
    fn cleanup_running(&self) -> bool {
        false
    }
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
    /// The loaded groups, re-filtered on every reload while they are unchanged (ADR-0133).
    cache: ps_service::review::ReviewCache,
    /// The loaded stack inputs, recomputed into stacks on every threshold change (ADR-0135).
    stacks: ps_service::stacks::StackCache,
}

impl CatalogReview {
    pub fn new(backend: Arc<CatalogBackend>) -> Self {
        Self {
            backend,
            review: std::sync::OnceLock::new(),
            ai: Mutex::new(None),
            cache: ps_service::review::ReviewCache::new(),
            stacks: ps_service::stacks::StackCache::new(),
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
    fn progress(&self) -> anyhow::Result<ReviewProgress> {
        // From the review cache: the catalog is read again only when the groups changed (ADR-0133).
        Ok(self.cache.progress(self.backend.scan()?, self.store()?)?)
    }
    fn groups(&self, filter: ReviewFilter, sort: ReviewSort) -> anyhow::Result<ReviewList> {
        let (scan, store) = (self.backend.scan()?, self.store()?);
        if !filter.has_ai() {
            return Ok(self.cache.list(scan, store, filter, sort, None)?);
        }
        // The gallery's insight index (built once, updated in place while the AI queue runs), so this list
        // and the gallery agree on the same filter (ai-filtered-duplicates D1).
        let mut sets = self.backend.insight_matches_many(&[
            filter.insight.unwrap_or_default(),
            ps_model::library::InsightCriteria::default(),
        ])?;
        let matcher = ps_service::review::IndexMatcher {
            described: sets.pop().unwrap_or_default(),
            matching: sets.pop().unwrap_or_default(),
        };
        Ok(self.cache.list(scan, store, filter, sort, Some(&matcher))?)
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
    fn cleanup_running(&self) -> bool {
        self.backend.cleanup_running()
    }
    fn set_reviewed(&self, group: FileId, reviewed: bool) -> anyhow::Result<bool> {
        Ok(ps_service::review::set_reviewed(
            self.backend.scan()?,
            self.store()?,
            group,
            reviewed,
        )?)
    }
    fn exclude_group(
        &self,
        group: FileId,
        reason: Option<String>,
    ) -> anyhow::Result<GroupExclusion> {
        let x =
            ps_service::review::exclude_group(self.backend.scan()?, self.store()?, group, reason)?;
        // A decision moves files between excluded and pending.
        self.backend.library().forget_dispositions();
        Ok(x)
    }
    fn include_group(
        &self,
        group: FileId,
        undo: Option<GroupExclusion>,
    ) -> anyhow::Result<ReviewDetail> {
        let d = ps_service::review::include_group(
            self.backend.scan()?,
            self.store()?,
            group,
            undo.as_ref(),
        )?;
        self.backend.library().forget_dispositions();
        Ok(d)
    }
    fn stacks(
        &self,
        t: ps_model::StackThresholds,
    ) -> anyhow::Result<std::sync::Arc<ps_service::stacks::StacksView>> {
        Ok(std::sync::Arc::new(self.stacks.view(
            self.backend.scan()?,
            self.store()?,
            t,
        )?))
    }
    fn stack_pick(&self, t: ps_model::StackThresholds, file: FileId) -> anyhow::Result<()> {
        let scan = self.backend.scan()?;
        let v = self.stacks.view(scan, self.store()?, t)?;
        ps_service::stacks::pick(scan, &v, file)?;
        self.backend.library().forget_dispositions();
        Ok(())
    }
    fn stack_take_out(&self, t: ps_model::StackThresholds, file: FileId) -> anyhow::Result<()> {
        let scan = self.backend.scan()?;
        let v = self.stacks.view(scan, self.store()?, t)?;
        ps_service::stacks::take_out(scan, &v, file)?;
        self.backend.library().forget_dispositions();
        Ok(())
    }
    fn stack_exclude(
        &self,
        t: ps_model::StackThresholds,
        index: usize,
    ) -> anyhow::Result<ps_service::stacks::StackExclusion> {
        let scan = self.backend.scan()?;
        let v = self.stacks.view(scan, self.store()?, t)?;
        let x = ps_service::stacks::exclude_stack(scan, &v, index, None)?;
        self.backend.library().forget_dispositions();
        Ok(x)
    }
    fn stack_undo_exclude(&self, x: ps_service::stacks::StackExclusion) -> anyhow::Result<()> {
        ps_service::stacks::undo_exclude_stack(self.backend.scan()?, &x)?;
        self.backend.library().forget_dispositions();
        Ok(())
    }
    fn stack_put_back(&self, file: FileId) -> anyhow::Result<()> {
        ps_service::stacks::put_back(self.backend.scan()?, file)?;
        self.backend.library().forget_dispositions();
        Ok(())
    }
    fn stacks_reviewed(
        &self,
        anchors: Vec<ps_model::ContentId>,
    ) -> anyhow::Result<std::collections::HashSet<ps_model::ContentId>> {
        let store = self.store()?;
        Ok(anchors
            .into_iter()
            .filter(|a| ps_service::review::stack_reviewed(store, a))
            .collect())
    }
    fn set_stack_reviewed(
        &self,
        anchor: ps_model::ContentId,
        reviewed: bool,
    ) -> anyhow::Result<()> {
        Ok(ps_service::review::set_stack_reviewed(
            self.store()?,
            &anchor,
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
        let _edit = self.backend.edit()?;
        Ok(ps_service::insights::decide(
            self.backend.catalog()?,
            self.istore()?,
            content,
            accept,
        )?)
    }
    fn ai_undo_decision(&self, content: &str) -> anyhow::Result<usize> {
        let _edit = self.backend.edit()?;
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

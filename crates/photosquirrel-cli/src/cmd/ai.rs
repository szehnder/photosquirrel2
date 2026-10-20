//! `ai providers|test|start|pause|status|results` (ADR-0020, ADR-0021: nothing is sent before `ai start`;
//! insights are advisory). The queue lives in the catalog, so `ai start` after Ctrl-C or a crash resumes.

use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use serde::Serialize;

use ps_service::ai::{self, AiService, AiStatus};
use ps_service::secrets::Secrets;

use super::Ctx;
use crate::control::{self, Interrupted, Job};
use crate::out::{emit, line};

#[derive(Serialize)]
struct PresetOut {
    id: &'static str,
    label: &'static str,
    base_url: &'static str,
    hosted: bool,
    env_var: Option<&'static str>,
}

#[derive(Serialize)]
struct ProvidersOut {
    configured: String,
    base_url: String,
    model: String,
    hosted: bool,
    consent: bool,
    requests_per_minute: Option<u32>,
    presets: Vec<PresetOut>,
}

pub fn providers(ctx: &Ctx) -> Result<()> {
    let s = ctx.settings()?;
    let out = ProvidersOut {
        configured: s.ai.provider.label().into(),
        base_url: s.ai.base_url.clone(),
        model: s.ai.model.clone(),
        hosted: s.ai.provider.is_hosted(),
        consent: ps_service::ai::consent_covers(&s.ai),
        requests_per_minute: s.ai.effective_requests_per_minute(),
        presets: ps_llm::PRESETS
            .iter()
            .map(|p| PresetOut {
                id: p.id,
                label: p.label,
                base_url: p.base_url,
                hosted: p.hosted,
                env_var: p.env_var,
            })
            .collect(),
    };
    emit(ctx.json, "ai providers", &out, |o| {
        println!(
            "configured: {} at {} (model {}){}",
            o.configured,
            o.base_url,
            if o.model.is_empty() {
                "loaded"
            } else {
                &o.model
            },
            if o.hosted { ", hosted" } else { ", local" }
        );
        for p in &o.presets {
            println!(
                "  {:<18} {:<28} {}{}",
                p.id,
                p.label,
                p.base_url,
                if p.hosted {
                    " (hosted: needs consent)"
                } else {
                    ""
                }
            );
        }
    })
}

#[derive(Serialize)]
struct TestOut {
    provider: String,
    reachable: bool,
    latency_ms: Option<u64>,
    error: Option<String>,
    model: Option<String>,
    model_found: Option<bool>,
    accepts_images: Option<bool>,
    models: Vec<String>,
}

pub fn test(ctx: &Ctx) -> Result<()> {
    let s = ctx.settings()?;
    let r = ai::test_connection(&s, &Secrets::os());
    let out = TestOut {
        provider: r.provider,
        reachable: r.reachable,
        latency_ms: r.latency.map(|d| d.as_millis() as u64),
        error: r.error,
        model: r.model,
        model_found: r.model_found,
        accepts_images: r.accepts_images,
        models: r.models,
    };
    let ok = out.reachable;
    emit(ctx.json, "ai test", &out, |o| {
        if o.reachable {
            println!(
                "{} is reachable ({} ms); model {} {}",
                o.provider,
                o.latency_ms.unwrap_or(0),
                o.model.as_deref().unwrap_or("?"),
                match o.model_found {
                    Some(true) => "is available",
                    Some(false) => "is NOT available",
                    None => "(not listed)",
                }
            );
        } else {
            println!(
                "{} is not reachable: {}",
                o.provider,
                o.error.as_deref().unwrap_or("unknown error")
            );
        }
    })?;
    if !ok {
        bail!("the provider is not reachable");
    }
    Ok(())
}

/// `ai start`: queue and describe in the foreground until done. Ctrl-C pauses (in-flight requests finish).
pub fn start(ctx: &Ctx, include_non_keepers: bool) -> Result<()> {
    let data = ctx.data_dir()?;
    if control::is_live(&data, Job::Ai) {
        bail!("the AI queue is already running in another terminal");
    }
    let settings = ctx.settings()?;
    let svc = ctx.service()?;
    let ai = AiService::from_settings(&svc, &settings, &Secrets::os())?;
    let queued = ai.start(include_non_keepers)?;
    log::info!("queued {queued} new items");
    let mut last = Instant::now() - Duration::from_secs(10);
    let mut paused = false;
    loop {
        if (control::interrupted()
            || control::take_request(&data, Job::Ai).as_deref() == Some("pause"))
            && !paused
        {
            ai.pause()?;
            paused = true;
        }
        let st = ai.status()?;
        control::beat(&data, Job::Ai, &st);
        if last.elapsed() >= Duration::from_millis(500) {
            if ctx.json {
                line("ai start", "snapshot", &st)?;
            } else {
                eprintln!(
                    "AI queue {}: {} done, {} pending, {} in flight, {} failed",
                    st.state, st.done, st.pending, st.in_flight, st.failed
                );
            }
            last = Instant::now();
        }
        if ai.wait_idle(Duration::from_millis(200)) {
            break;
        }
    }
    let st = ai.status()?;
    control::end(&data, Job::Ai);
    if ctx.json {
        line("ai start", "summary", &st)?;
    } else {
        println!(
            "AI queue {}: {} done, {} pending, {} failed, {} results stored",
            st.state, st.done, st.pending, st.failed, st.results
        );
    }
    if paused && control::interrupted() {
        return Err(Interrupted.into());
    }
    Ok(())
}

pub fn pause(ctx: &Ctx) -> Result<()> {
    let data = ctx.data_dir()?;
    if control::is_live(&data, Job::Ai) {
        control::request(&data, Job::Ai, "pause")?;
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(10) && control::is_live(&data, Job::Ai) {
            let st: Option<AiStatus> = control::live(&data, Job::Ai);
            if st.as_ref().is_some_and(|s| s.state != "running") {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    } else {
        let svc = ctx.service()?;
        svc.engine()
            .catalog()
            .writer()
            .ai_set_run(ps_catalog::AiRunState::Paused, None, None)?;
    }
    status(ctx)
}

pub fn status(ctx: &Ctx) -> Result<()> {
    let data = ctx.data_dir()?;
    let svc = ctx.reader()?;
    let mut st = ai::status(svc.engine().catalog())?;
    if let Some(live) = control::live::<AiStatus>(&data, Job::Ai) {
        st.state = live.state;
        st.per_minute = live.per_minute;
        st.eta_s = live.eta_s;
    }
    emit(ctx.json, "ai status", &st, |s| {
        println!(
            "AI queue {} ({}): {} done, {} pending, {} in flight, {} failed, {} deferred; {} results",
            s.state,
            s.provider.as_deref().unwrap_or("no provider yet"),
            s.done,
            s.pending,
            s.in_flight,
            s.failed,
            s.deferred,
            s.results
        );
        if let Some(r) = s.per_minute {
            println!(
                "{r:.1} items/min{}",
                s.eta_s.map_or_else(String::new, |e| format!(", ETA {e} s"))
            );
        }
    })
}

pub fn results(ctx: &Ctx, limit: u64) -> Result<()> {
    let svc = ctx.reader()?;
    let r = ai::results(svc.engine().catalog(), limit)?;
    emit(ctx.json, "ai results", &r, |r| {
        if r.is_empty() {
            println!("no insights yet: photosquirrel-cli ai start");
        }
        for x in r {
            println!(
                "{} ({} files) {}/{} prompt v{}{}: {}",
                &x.content[..12],
                x.files.len(),
                x.provider,
                x.model,
                x.prompt_version,
                if x.older_prompt { " (older)" } else { "" },
                serde_json::to_string(&x.insight).unwrap_or_default()
            );
        }
    })
}

#[derive(serde::Serialize)]
struct RedescribeOut {
    queued: u64,
    prompt_version: u32,
}

/// `ai redescribe`: queue the results of older prompts again (explicit; nothing is sent here).
pub fn redescribe(ctx: &Ctx) -> Result<()> {
    let svc = ctx.service()?;
    let queued = ai::redescribe_older_prompts(svc.engine().catalog())?;
    let out = RedescribeOut {
        queued,
        prompt_version: ps_llm::PROMPT_VERSION,
    };
    emit(ctx.json, "ai redescribe", &out, |o| {
        if o.queued == 0 {
            println!(
                "every insight is from the current prompt (v{})",
                o.prompt_version
            );
        } else {
            println!(
                "queued {} photos described with an older prompt for prompt v{}; run `photosquirrel-cli ai start` to describe them",
                o.queued, o.prompt_version
            );
        }
    })
}

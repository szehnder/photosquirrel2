//! `doctor`: version, data folder, catalog schema, decoders per format with the reason and fix for this system
//! (ADR-0089, ADR-0090), keychain (ADR-0031) and the
//! configured AI provider, and the power probe behind pause on battery. Opens no network connection (ADR-0029) and creates nothing.

use std::path::PathBuf;

use anyhow::Result;
use serde::Serialize;

use ps_service::power::PowerSource as _;
use ps_service::secrets::{Keychain, SecretBackend};

use super::Ctx;
use crate::out::emit;

/// One format's decoder: `name`, `available` and `detail` as before, plus what detection found (ADR-0089).
#[derive(Serialize)]
struct Decoder {
    name: &'static str,
    available: bool,
    detail: String,
    backend: Option<String>,
    version: Option<String>,
    reason: Option<String>,
    fix: Option<ps_model::Fix>,
    /// AVIF: built with rav1d's assembly (`avif-asm`).
    #[serde(skip_serializing_if = "Option::is_none")]
    assembly: Option<bool>,
}

#[derive(Serialize)]
struct Catalog {
    path: PathBuf,
    exists: bool,
    schema_version: Option<u32>,
    supported_schema_version: u32,
    error: Option<String>,
}

#[derive(Serialize)]
struct Provider {
    name: String,
    base_url: String,
    model: String,
    hosted: bool,
    hosted_consent: bool,
    requests_per_minute: Option<u32>,
}

/// The power probe that pause-on-battery uses (ADR-0053, ADR-0072).
#[derive(Serialize)]
struct Power {
    /// `"system"`, or `"override"` with `PHOTOSQUIRREL_POWER_SUPPLY_DIR`.
    probe: &'static str,
    override_dir: Option<PathBuf>,
    /// The override folder exists (a missing one reads as mains power).
    override_dir_exists: Option<bool>,
    on_battery: bool,
    pause_on_battery: bool,
}

#[derive(Serialize)]
struct DoctorOut {
    version: &'static str,
    data_dir: PathBuf,
    settings: PathBuf,
    catalog: Catalog,
    heic: bool,
    decoders: Vec<Decoder>,
    /// `PHOTOSQUIRREL_HEIF_DECODER` / `PHOTOSQUIRREL_LIBHEIF` in force.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    decoder_override: Vec<String>,
    os: Option<ps_model::OsSummary>,
    keychain_available: bool,
    keychain_detail: String,
    llm_provider: Provider,
    power: Power,
    network_used: bool,
}

pub fn run(ctx: &Ctx) -> Result<()> {
    let data = ctx.data_dir()?;
    let store = ctx.settings_store()?;
    let settings = store.load().settings;
    let path = ps_catalog::db::catalog_path(&data);
    let (version, error) = match ps_catalog::db::stored_version(&path) {
        Ok(v) => (v, None),
        Err(e) => (None, Some(e.to_string())),
    };
    // Detection: one library load and two 1 KB decodes, reading no user file (ADR-0029).
    let report = ps_media::decoder_report(
        &ps_service::decoders::DecoderHub::global().get(),
        settings
            .scanning
            .poster_frame_tool
            .clone()
            .or_else(ps_media::poster::ffmpeg_on_path)
            .as_deref(),
    );
    let (keychain_available, keychain_detail) = match Keychain.get("photosquirrel-doctor-probe") {
        Ok(_) => (true, "OS keychain reachable".to_owned()),
        Err(e) => (
            false,
            format!("{e}; API keys are kept for this session only"),
        ),
    };
    let out = DoctorOut {
        version: env!("CARGO_PKG_VERSION"),
        settings: store.path(),
        catalog: Catalog {
            exists: path.exists(),
            path,
            schema_version: version,
            supported_schema_version: ps_catalog::latest_schema_version(),
            error,
        },
        data_dir: data,
        heic: report
            .entry(ps_model::DecoderFormat::Heic)
            .is_some_and(|e| e.available),
        decoders: report
            .entries
            .iter()
            .map(|e| Decoder {
                name: e.format.as_str(),
                available: e.available,
                detail: e.detail(),
                backend: e.backend.clone(),
                version: e.version.clone(),
                reason: e.reason.clone(),
                fix: e.fix.clone(),
                assembly: e.assembly,
            })
            .collect(),
        decoder_override: report.overrides.clone(),
        os: report.os.clone(),
        keychain_available,
        keychain_detail,
        llm_provider: Provider {
            name: settings.ai.provider.label().into(),
            base_url: settings.ai.base_url.clone(),
            model: settings.ai.model.clone(),
            hosted: settings.ai.provider.is_hosted(),
            hosted_consent: ps_service::ai::consent_covers(&settings.ai),
            requests_per_minute: settings.ai.effective_requests_per_minute(),
        },
        power: {
            let dir = ps_service::power::override_dir();
            Power {
                probe: if dir.is_some() { "override" } else { "system" },
                override_dir_exists: dir.as_ref().map(|d| d.is_dir()),
                on_battery: match &dir {
                    Some(root) => ps_service::power::SysfsPower { root: root.clone() }.on_battery(),
                    None => ps_service::power::SystemPower.on_battery(),
                },
                override_dir: dir,
                pause_on_battery: settings.general.pause_on_battery,
            }
        },
        network_used: false,
    };
    emit(ctx.json, "doctor", &out, |o| {
        println!("photosquirrel-cli {}", o.version);
        println!("data folder: {}", o.data_dir.display());
        println!("settings:    {}", o.settings.display());
        match (o.catalog.exists, o.catalog.schema_version) {
            (true, Some(v)) => println!(
                "catalog:     {} (schema {v}, this build supports {})",
                o.catalog.path.display(),
                o.catalog.supported_schema_version
            ),
            _ => println!(
                "catalog:     not created yet ({}){}",
                o.catalog.path.display(),
                o.catalog
                    .error
                    .as_deref()
                    .map_or(String::new(), |e| format!(": {e}"))
            ),
        }
        for d in &o.decoders {
            println!(
                "decoder {:<14} {}  {}",
                d.name,
                if d.available {
                    "available  "
                } else {
                    "unavailable"
                },
                d.detail
            );
            if let Some(fix) = d.fix.as_ref().filter(|_| !d.available) {
                if !fix.summary.is_empty() {
                    println!("    how to enable: {}", fix.summary);
                }
                for l in fix.lines() {
                    println!("      {l}");
                }
            }
        }
        for v in &o.decoder_override {
            println!("decoder override: {v}");
        }
        println!(
            "keychain:    {}",
            if o.keychain_available {
                "available"
            } else {
                "unavailable"
            }
        );
        println!(
            "AI provider: {} at {} ({}{})",
            o.llm_provider.name,
            o.llm_provider.base_url,
            if o.llm_provider.hosted {
                "hosted"
            } else {
                "local"
            },
            if o.llm_provider.hosted && !o.llm_provider.hosted_consent {
                ", no consent yet"
            } else {
                ""
            }
        );
        let p = &o.power;
        println!(
            "power:       {}, {}; pause scans on battery {}",
            match (&p.override_dir, p.override_dir_exists) {
                (Some(d), Some(false)) => format!(
                    "override {} (PHOTOSQUIRREL_POWER_SUPPLY_DIR), missing: reads as mains",
                    d.display()
                ),
                (Some(d), _) =>
                    format!("override {} (PHOTOSQUIRREL_POWER_SUPPLY_DIR)", d.display()),
                (None, _) => "system probe".to_owned(),
            },
            if p.on_battery {
                "on battery"
            } else {
                "on mains"
            },
            if p.pause_on_battery { "on" } else { "off" }
        );
    })
}

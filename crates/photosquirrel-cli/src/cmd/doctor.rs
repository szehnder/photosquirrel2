//! `doctor`: version, data folder, catalog schema, optional decoders (ADR-0017), keychain (ADR-0031) and the
//! configured AI provider. Opens no network connection (ADR-0029) and creates nothing.

use std::path::PathBuf;

use anyhow::Result;
use serde::Serialize;

use ps_service::secrets::{Keychain, SecretBackend};

use super::Ctx;
use crate::out::emit;

#[derive(Serialize)]
struct Decoder {
    name: &'static str,
    available: bool,
    detail: String,
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

#[derive(Serialize)]
struct DoctorOut {
    version: &'static str,
    data_dir: PathBuf,
    settings: PathBuf,
    catalog: Catalog,
    heic: bool,
    decoders: Vec<Decoder>,
    keychain_available: bool,
    keychain_detail: String,
    llm_provider: Provider,
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
    let caps = ps_media::capabilities(
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
        heic: caps.heic,
        decoders: caps
            .decoders
            .into_iter()
            .map(|d| Decoder {
                name: d.name,
                available: d.available,
                detail: d.detail,
            })
            .collect(),
        keychain_available,
        keychain_detail,
        llm_provider: Provider {
            name: settings.ai.provider.label().into(),
            base_url: settings.ai.base_url.clone(),
            model: settings.ai.model.clone(),
            hosted: settings.ai.provider.is_hosted(),
            hosted_consent: settings.ai.hosted_consent,
            requests_per_minute: settings.ai.effective_requests_per_minute(),
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
    })
}

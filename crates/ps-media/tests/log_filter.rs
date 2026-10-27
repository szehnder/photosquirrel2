//! `cli` scenario of `quiet-decoder-logs`: the default log filter both binaries start with.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use log::{Level, Log, Metadata};

/// Whether a logger built like the binaries' (with `RUST_LOG` unset) lets `level` through for `target`.
fn enabled(level: Level, target: &str) -> bool {
    let logger = env_logger::Builder::new()
        .parse_filters(ps_media::LOG_DEFAULT_FILTER)
        .build();
    logger.enabled(&Metadata::builder().level(level).target(target).build())
}

#[test]
fn scenario_cli_the_default_filter_keeps_errors_and_our_own_warnings() {
    for target in [
        "rawler",
        "rawler::lens",
        "rawler::decoders",
        "rawler::formats::tiff::ifd",
        "nom_exif",
        "nom_exif::parser",
    ] {
        assert!(!enabled(Level::Warn, target), "{target} warnings shown");
        assert!(enabled(Level::Error, target), "{target} errors hidden");
    }
    for target in [
        "ps_scan",
        "ps_media",
        "ps_service",
        "photosquirrel_cli",
        "photosquirrel",
    ] {
        assert!(enabled(Level::Warn, target), "{target} warnings hidden");
        assert!(enabled(Level::Error, target), "{target} errors hidden");
    }
    assert!(!enabled(Level::Info, "ps_scan"));
}

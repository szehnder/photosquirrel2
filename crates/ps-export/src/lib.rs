//! Copy-only verified export (ADR-0004, ADR-0022, ADR-0023) and reversible quarantine cleanup (ADR-0024).
//!
//! The only crate allowed to create files under user paths: inside the chosen library folder, and (cleanup
//! only) the quarantine folder of a source:
//!
//! - [`input`]: the pure export input (keeper sets with metadata, insights and every copy's source path). The
//!   service builds it from the catalog and the dedup grouping; tests build it directly.
//! - [`date`]: the filing date of a keeper (its own EXIF, else the best date its duplicate group knows).
//! - [`plan`]: the dry run. Keepers minus the ledger, the dated layout ([`layout`]), collision suffixes,
//!   adoption of identical files already in the library, RAW+JPEG and Live Photo bundles ([`pairs`]),
//!   offline sources, the library/source overlap check and the free-space check. Planning only reads.
//! - [`copy`]: the copy protocol: stream to `<name>.ps-partial` while hashing with BLAKE3, fsync, read back
//!   and verify against the catalog identity, preserve the mtime, then rename with no-replace semantics
//!   ([`rename`]) and fsync the directory.
//! - [`execute`]: runs a confirmed plan with the ledger ([`ledger`]): idempotent and resumable.
//! - [`xmp`] sidecars, the append-only [`manifest`], [`profile`] defaults.
//! - [`disposition`]: derived dispositions and completion reports (ADR-0023).
//! - [`quarantine`]: reversible cleanup (ADR-0024, ADR-0061): the only code that renames files in a source
//!   tree, by a same-volume no-replace rename into its quarantine folder and back, journaled. Never deletes.
//!
//! Safety invariants (tested in `tests/copy.rs`: `scenario_export_export_leaves_sources_untouched`,
//! `scenario_export_existing_file_never_overwritten`, the kill and disk-full tests): sources are opened read-only
//! and never changed; nothing is written outside the library root; no existing library file is ever replaced
//! or modified; the only files ever removed are this exporter's own `.ps-partial` files inside the library.

pub mod copy;
pub mod date;
pub mod disposition;
pub mod execute;
pub mod input;
pub mod layout;
pub mod ledger;
pub mod library;
pub mod manifest;
pub mod pairs;
pub mod plan;
pub mod profile;
pub mod quarantine;
pub mod rename;
pub mod xmp;

pub use copy::{CopyConfig, CopyError, CopyHooks, CopyOutcome, NoHooks};
pub use date::{DateFacts, DateOrigin, DateSource, FilingDate};
pub use disposition::{
    Badge, CompletionReport, CopyCheck, DispositionFacts, FileDisposition, FileFacts, FolderReport,
    GroupFacts, SUPERSEDED_HINT, SourceReport, check_library, copy_check_text, count_disposition,
    derive_dispositions, report, superseded_label,
};
pub use execute::{ExecConfig, ExecError, ExportReport, FailedFile, StopReason, execute};
pub use input::{CopyRef, ExportInput, ExportUnit, KeeperFile, SourceInfo};
pub use ledger::{Ledger, MemoryLedger};
pub use library::{Existing, FsLibrary, LibraryProbe, LibraryRoot, PARTIAL_SUFFIX};
pub use plan::{
    AlreadyExported, BetterCopy, BetterCopyKeeper, BetterCopyLibrary, BundleKind, DateTotals,
    ModifiedInLibrary, Plan, PlanBlocker, PlanError, PlanTotals, PlanWarning, PlannedAction,
    PlannedBundle, PlannedFile, Unavailable, plan,
};
pub use profile::ProfileSettings;
pub use rename::RenameMethod;

/// Directory inside the library holding PhotoSquirrel's own files (manifest).
pub const LIBRARY_META_DIR: &str = ".photosquirrel";

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

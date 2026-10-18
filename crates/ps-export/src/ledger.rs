//! The export ledger (ADR-0023): one row per library path, written `in_progress` before a partial file is
//! created and `done` after the verified rename.
//!
//! The durable implementation lives in the catalog (`export_ledger`, migration 6) behind the single writer;
//! wiring it is the export change's integration step. [`Ledger::record`] must be durable when it returns.

use std::collections::BTreeMap;
use std::io;

use ps_model::{ContentId, LedgerEntry, LedgerState, RelPath};

/// Storage for ledger rows of one library.
pub trait Ledger: Send {
    /// Every row.
    fn entries(&self) -> io::Result<Vec<LedgerEntry>>;
    /// Insert or replace the row at `entry.library_rel_path`; durable on return.
    fn record(&mut self, entry: &LedgerEntry) -> io::Result<()>;
}

/// In-memory ledger (tests, dry runs).
#[derive(Clone, Debug, Default)]
pub struct MemoryLedger {
    rows: BTreeMap<RelPath, LedgerEntry>,
}

impl MemoryLedger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_entries(entries: impl IntoIterator<Item = LedgerEntry>) -> Self {
        let mut l = Self::new();
        for e in entries {
            l.rows.insert(e.library_rel_path.clone(), e);
        }
        l
    }

    pub fn get(&self, path: &RelPath) -> Option<&LedgerEntry> {
        self.rows.get(path)
    }

    pub fn done(&self) -> impl Iterator<Item = &LedgerEntry> {
        self.rows.values().filter(|e| e.state == LedgerState::Done)
    }

    /// Done rows of a content.
    pub fn done_for(&self, content: &ContentId) -> impl Iterator<Item = &LedgerEntry> {
        let c = *content;
        self.done().filter(move |e| e.content == c)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

impl Ledger for MemoryLedger {
    fn entries(&self) -> io::Result<Vec<LedgerEntry>> {
        Ok(self.rows.values().cloned().collect())
    }

    fn record(&mut self, entry: &LedgerEntry) -> io::Result<()> {
        self.rows
            .insert(entry.library_rel_path.clone(), entry.clone());
        Ok(())
    }
}

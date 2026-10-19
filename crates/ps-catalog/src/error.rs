use std::path::PathBuf;

/// Errors from opening, writing or querying the catalog and the thumbnail store.
#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("i/o on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(
        "{path} has schema version {found}, but this build supports at most version {supported}; \
         it was written by a newer PhotoSquirrel and was not modified"
    )]
    NewerSchema {
        path: PathBuf,
        found: u32,
        supported: u32,
    },

    #[error("no per-user data directory is available; set PHOTOSQUIRREL_DATA_DIR")]
    NoDataDir,

    #[error("the catalog writer has stopped")]
    WriterClosed,

    #[error(
        "another PhotoSquirrel process (the app or a CLI command) is using the catalog in {}; \
         close it or wait for it to finish, then try again",
        .path.display()
    )]
    Locked { path: PathBuf },

    #[error("the catalog is open read-only because another PhotoSquirrel process owns it")]
    ReadOnly,

    #[error("{} catalog write error(s); first: {}", .0.total, .0.first())]
    Write(WriteErrors),

    #[error("invalid data in the catalog: {0}")]
    Corrupt(String),

    #[error("invalid argument: {0}")]
    Invalid(String),
}

pub type Result<T, E = CatalogError> = std::result::Result<T, E>;

/// One failed write (a row that could not be applied, or a failed commit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteError {
    /// The command kind, e.g. `upsert_files`, `analyzed`, `commit`.
    pub command: &'static str,
    /// The file the row was about, when there is one.
    pub file: Option<crate::FileKey>,
    pub message: String,
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.file {
            Some(k) => write!(
                f,
                "{} {}:{}: {}",
                self.command, k.source, k.path, self.message
            ),
            None => write!(f, "{}: {}", self.command, self.message),
        }
    }
}

/// Write errors collected since the previous flush (the first [`WriteErrors::KEEP`] are kept verbatim).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WriteErrors {
    pub errors: Vec<WriteError>,
    pub total: usize,
}

impl WriteErrors {
    pub const KEEP: usize = 100;

    pub(crate) fn push(&mut self, e: WriteError) {
        log::warn!("catalog write error: {e}");
        if self.errors.len() < Self::KEEP {
            self.errors.push(e);
        }
        self.total += 1;
    }

    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    fn first(&self) -> String {
        self.errors
            .first()
            .map(ToString::to_string)
            .unwrap_or_default()
    }
}

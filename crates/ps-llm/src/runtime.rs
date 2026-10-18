//! The tokio runtime owned by `ps-llm` (ADR-0009: tokio only for network I/O). Callers are plain threads; they
//! use [`LlmRuntime::block_on`] or spawn onto [`LlmRuntime::handle`].

use std::future::Future;
use std::sync::OnceLock;

/// A small multi-threaded runtime for provider requests.
#[derive(Debug)]
pub struct LlmRuntime {
    rt: tokio::runtime::Runtime,
}

impl LlmRuntime {
    pub fn new() -> std::io::Result<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("ps-llm")
            .enable_all()
            .build()?;
        Ok(Self { rt })
    }

    /// The process-wide runtime, created on first use.
    pub fn shared() -> &'static LlmRuntime {
        static RT: OnceLock<LlmRuntime> = OnceLock::new();
        #[allow(clippy::expect_used)]
        RT.get_or_init(|| LlmRuntime::new().expect("cannot start the ps-llm runtime"))
    }

    pub fn handle(&self) -> tokio::runtime::Handle {
        self.rt.handle().clone()
    }

    /// Run a future to completion from a non-runtime thread.
    pub fn block_on<F: Future>(&self, f: F) -> F::Output {
        self.rt.block_on(f)
    }
}

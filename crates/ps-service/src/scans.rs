//! Scan dashboard support (change `scan-dashboard`): the ETA estimator ([`eta`]), run summaries and the full
//! error listing ([`summary`]), and a simulated engine for UI tests and screenshots ([`fake`]).

pub mod eta;
pub mod fake;
pub mod summary;

pub use eta::{Eta, EtaEstimator};
pub use fake::{FakeConfig, FakeScanService, FakeSource};
pub use summary::{RunError, RunOverview};

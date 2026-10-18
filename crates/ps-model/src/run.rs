//! Source and scan-run states (ADR-0006, ADR-0008).

use serde::{Deserialize, Serialize};

/// Whether a source's root is currently reachable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceStatus {
    #[default]
    Online,
    /// The root is missing (unplugged drive, unmounted share); history is kept.
    Offline,
}

impl SourceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceStatus::Online => "online",
            SourceStatus::Offline => "offline",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "online" => SourceStatus::Online,
            "offline" => SourceStatus::Offline,
            _ => return None,
        })
    }
}

/// Status of a scan run (ADR-0008).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Paused,
    Cancelled,
    Completed,
    /// Left `running` by a crash; offered for resume.
    Interrupted,
}

impl RunStatus {
    pub const ALL: [RunStatus; 5] = [
        RunStatus::Running,
        RunStatus::Paused,
        RunStatus::Cancelled,
        RunStatus::Completed,
        RunStatus::Interrupted,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            RunStatus::Running => "running",
            RunStatus::Paused => "paused",
            RunStatus::Cancelled => "cancelled",
            RunStatus::Completed => "completed",
            RunStatus::Interrupted => "interrupted",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        RunStatus::ALL.into_iter().find(|r| r.as_str() == s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_round_trip() {
        for r in RunStatus::ALL {
            assert_eq!(RunStatus::parse(r.as_str()), Some(r));
        }
        for s in [SourceStatus::Online, SourceStatus::Offline] {
            assert_eq!(SourceStatus::parse(s.as_str()), Some(s));
        }
    }
}

//! Persistence mode — domain control-plane unifying `--resume`/`--state-dir`
//! and `--checkpoint-interval`/`--no-checkpoint` without changing persisted formats.
//!
//! Pure resolver: `PersistenceMode::from_limits(&CrawlLimits, &Path)` maps four
//! CLI flags to an exhaustive enum. No IO, no async, deterministic.

use std::path::{Path, PathBuf};

use tracing::warn;

use crate::application::crawl_options::CrawlLimits;

/// Checkpoint configuration — directory and interval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointCfg {
    /// Directory where `crawl_checkpoint.json` lives.
    pub dir: PathBuf,
    /// Pages between automatic checkpoint saves (0 is disabled, but this struct
    /// is only constructed when checkpoint is enabled, so interval > 0 here).
    pub interval: u64,
}

/// Unified persistence control-plane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersistenceMode {
    /// No persistence active.
    Disabled,
    /// Resume only (`RecordStore` via `apply_resume_mode`).
    Resume {
        /// Effective resume state directory.
        dir: PathBuf,
    },
    /// Checkpoint only (`Engine::with_checkpoint`).
    Checkpoint {
        /// Checkpoint configuration.
        cfg: CheckpointCfg,
    },
    /// Both resume and checkpoint active.
    Full {
        /// Effective resume state directory.
        resume_dir: PathBuf,
        /// Checkpoint configuration.
        checkpoint: CheckpointCfg,
    },
}

impl PersistenceMode {
    /// Pure resolver — no IO.
    ///
    /// `default_state_dir` is the caller-supplied fallback (XDG_CACHE_HOME or
    /// `~/.cache/webfang/state`). `state_dir` without `--resume` is ignored
    /// (caller emits `warn!`).
    pub fn from_limits(limits: &CrawlLimits, default_state_dir: &Path) -> Self {
        if limits.state_dir.is_some() && !limits.resume {
            warn!(
                state_dir = ?limits.state_dir,
                "ignoring --state-dir without --resume"
            );
        }
        let checkpoint_enabled = limits.checkpoint_interval != 0 && !limits.no_checkpoint;
        let resume_enabled = limits.resume;

        match (resume_enabled, checkpoint_enabled) {
            (false, false) => Self::Disabled,
            (true, false) => {
                let dir = limits
                    .state_dir
                    .clone()
                    .unwrap_or_else(|| default_state_dir.to_path_buf());
                Self::Resume { dir }
            },
            (false, true) => {
                // --state-dir without --resume is ignored → default dir.
                let cfg = CheckpointCfg {
                    dir: default_state_dir.to_path_buf(),
                    interval: limits.checkpoint_interval,
                };
                Self::Checkpoint { cfg }
            },
            (true, true) => {
                let resume_dir = limits
                    .state_dir
                    .clone()
                    .unwrap_or_else(|| default_state_dir.to_path_buf());
                let checkpoint = CheckpointCfg {
                    dir: resume_dir.clone(),
                    interval: limits.checkpoint_interval,
                };
                Self::Full {
                    resume_dir,
                    checkpoint,
                }
            },
        }
    }

    /// `true` for `Resume` and `Full`.
    #[must_use]
    pub fn is_resume(&self) -> bool {
        matches!(self, Self::Resume { .. } | Self::Full { .. })
    }

    /// Checkpoint config when checkpoint is enabled.
    #[must_use]
    pub fn checkpoint_cfg(&self) -> Option<&CheckpointCfg> {
        match self {
            Self::Checkpoint { cfg }
            | Self::Full {
                checkpoint: cfg, ..
            } => Some(cfg),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn default_dir() -> PathBuf {
        PathBuf::from("/tmp/default_state")
    }

    fn limits(
        resume: bool,
        state_dir: Option<&str>,
        interval: u64,
        no_checkpoint: bool,
    ) -> CrawlLimits {
        CrawlLimits {
            resume,
            state_dir: state_dir.map(PathBuf::from),
            checkpoint_interval: interval,
            no_checkpoint,
            ..CrawlLimits::default()
        }
    }

    // ——— 8-combo matrix ———

    #[test]
    fn disabled_when_no_resume_and_checkpoint_disabled_by_zero() {
        let m = PersistenceMode::from_limits(&limits(false, None, 0, false), &default_dir());
        assert_eq!(m, PersistenceMode::Disabled);
    }

    #[test]
    fn disabled_when_no_resume_and_no_checkpoint_flag() {
        let m = PersistenceMode::from_limits(&limits(false, None, 100, true), &default_dir());
        assert_eq!(m, PersistenceMode::Disabled);
    }

    #[test]
    fn resume_only_with_default_dir() {
        let m = PersistenceMode::from_limits(&limits(true, None, 0, false), &default_dir());
        assert_eq!(m, PersistenceMode::Resume { dir: default_dir() });
    }

    #[test]
    fn resume_with_custom_state_dir() {
        let m = PersistenceMode::from_limits(
            &limits(true, Some("/tmp/cache"), 0, false),
            &default_dir(),
        );
        assert_eq!(
            m,
            PersistenceMode::Resume {
                dir: PathBuf::from("/tmp/cache")
            }
        );
    }

    #[test]
    fn checkpoint_only_with_default_dir() {
        let m = PersistenceMode::from_limits(&limits(false, None, 100, false), &default_dir());
        assert_eq!(
            m,
            PersistenceMode::Checkpoint {
                cfg: CheckpointCfg {
                    dir: default_dir(),
                    interval: 100
                }
            }
        );
    }

    #[test]
    fn checkpoint_with_custom_interval() {
        let m = PersistenceMode::from_limits(&limits(false, None, 50, false), &default_dir());
        assert_eq!(
            m,
            PersistenceMode::Checkpoint {
                cfg: CheckpointCfg {
                    dir: default_dir(),
                    interval: 50
                }
            }
        );
    }

    #[test]
    fn full_with_resume_and_checkpoint() {
        let m =
            PersistenceMode::from_limits(&limits(true, Some("/tmp/x"), 50, false), &default_dir());
        assert_eq!(
            m,
            PersistenceMode::Full {
                resume_dir: PathBuf::from("/tmp/x"),
                checkpoint: CheckpointCfg {
                    dir: PathBuf::from("/tmp/x"),
                    interval: 50
                }
            }
        );
    }

    #[test]
    fn full_with_default_dir_and_interval_100() {
        let m = PersistenceMode::from_limits(&limits(true, None, 100, false), &default_dir());
        assert_eq!(
            m,
            PersistenceMode::Full {
                resume_dir: default_dir(),
                checkpoint: CheckpointCfg {
                    dir: default_dir(),
                    interval: 100
                }
            }
        );
    }

    // ——— Disable rules ———

    #[test]
    fn interval_zero_disables_checkpoint_even_with_resume() {
        let m = PersistenceMode::from_limits(&limits(true, None, 0, false), &default_dir());
        // With interval 0, checkpoint must be disabled → Resume only, not Full.
        assert_eq!(m, PersistenceMode::Resume { dir: default_dir() });
        assert!(m.checkpoint_cfg().is_none());
    }

    #[test]
    fn no_checkpoint_disables_checkpoint() {
        let m = PersistenceMode::from_limits(&limits(true, None, 100, true), &default_dir());
        // no_checkpoint true → Resume only, interval ignored.
        assert_eq!(m, PersistenceMode::Resume { dir: default_dir() });
        assert!(m.checkpoint_cfg().is_none());
    }

    #[test]
    fn checkpoint_disabled_when_interval_zero_even_without_resume() {
        let m = PersistenceMode::from_limits(&limits(false, None, 0, false), &default_dir());
        assert_eq!(m, PersistenceMode::Disabled);
        assert!(m.checkpoint_cfg().is_none());
    }

    // ——— state_dir without --resume is ignored ———

    #[test]
    fn state_dir_without_resume_is_ignored_checkpoint_uses_default() {
        let m = PersistenceMode::from_limits(
            &limits(false, Some("/tmp/cache"), 100, false),
            &default_dir(),
        );
        // Should be Checkpoint with default dir, NOT /tmp/cache.
        assert_eq!(
            m,
            PersistenceMode::Checkpoint {
                cfg: CheckpointCfg {
                    dir: default_dir(),
                    interval: 100
                }
            }
        );
    }

    #[test]
    fn state_dir_without_resume_and_checkpoint_disabled_is_disabled() {
        let m = PersistenceMode::from_limits(
            &limits(false, Some("/tmp/cache"), 0, false),
            &default_dir(),
        );
        assert_eq!(m, PersistenceMode::Disabled);
    }

    // ——— helpers ———

    #[test]
    fn is_resume_helpers() {
        assert!(!PersistenceMode::Disabled.is_resume());
        assert!(PersistenceMode::Resume { dir: default_dir() }.is_resume());
        assert!(!PersistenceMode::Checkpoint {
            cfg: CheckpointCfg {
                dir: default_dir(),
                interval: 100
            }
        }
        .is_resume());
        assert!(PersistenceMode::Full {
            resume_dir: default_dir(),
            checkpoint: CheckpointCfg {
                dir: default_dir(),
                interval: 100
            }
        }
        .is_resume());
    }

    #[test]
    fn checkpoint_cfg_helper() {
        assert!(PersistenceMode::Disabled.checkpoint_cfg().is_none());
        assert!(PersistenceMode::Resume { dir: default_dir() }
            .checkpoint_cfg()
            .is_none());
        let cp = PersistenceMode::Checkpoint {
            cfg: CheckpointCfg {
                dir: default_dir(),
                interval: 42,
            },
        };
        assert_eq!(cp.checkpoint_cfg().unwrap().interval, 42);
        let full = PersistenceMode::Full {
            resume_dir: default_dir(),
            checkpoint: CheckpointCfg {
                dir: default_dir(),
                interval: 99,
            },
        };
        assert_eq!(full.checkpoint_cfg().unwrap().interval, 99);
    }
}

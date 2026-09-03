//! Persistence mode — domain control-plane unifying `--resume`/`--state-dir`
//! and `--checkpoint-interval`/`--no-checkpoint` without changing persisted formats.
//!
//! Pure resolver: `PersistenceMode::from_config(&ResumeConfig, &Path)` maps four
//! CLI flags to an exhaustive enum. No IO, no async, no logging, deterministic.
//! When the caller needs to know whether `--state-dir` was silently ignored,
//! use `from_config_with_notes` and act on the returned [`ResolverNotes`]
//! (the CLI layer re-emits the `warn!` from there; batch/MCP callers drop it).
//!
//! [`ResolverNotes`]: crate::domain::persistence::ResolverNotes
//!
//! The `ResumeConfig` value object is owned by domain (it is the input the
//! resolver consumes); the application layer (`CrawlLimits::resume_config`)
//! is responsible for translating CLI flags into a `ResumeConfig`. Domain
//! never imports from `crate::application::*` — the Clean Architecture rule
//! `infrastructure → adapters → application → domain` is inward only.

use std::path::{Path, PathBuf};

/// Input for [`PersistenceMode::from_config`].
///
/// Exactly the four CLI flags slice 5c unified. Held by domain so the
/// resolver stays independent of the application layer's `CrawlLimits`
/// (which carries ~12 unrelated composition fields: concurrency,
/// rate-limit, headers, cookies, patterns, etc.).
///
/// # Construction
///
/// Application code is the only place that names this type directly.
/// `CrawlLimits::resume_config()` is the canonical entry point. Tests
/// build it inline because the matrix is small.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResumeConfig {
    /// `--resume` (env `WEBFANG_RESUME`).
    pub resume: bool,
    /// `--state-dir <PATH>` (env `WEBFANG_STATE_DIR`).
    pub state_dir: Option<PathBuf>,
    /// `--checkpoint-interval <PAGES>` — 0 disables checkpointing.
    pub checkpoint_interval: u64,
    /// `--no-checkpoint` — explicit opt-out (overrides `checkpoint_interval`).
    pub no_checkpoint: bool,
}

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

/// Side-channel notes returned by [`PersistenceMode::from_config_with_notes`].
///
/// The resolver stays pure (no IO, no logging): every decision the caller
/// might need to surface is returned as data. Today that is exactly one
/// case — `--state-dir` without `--resume` is ignored. The CLI layer
/// re-emits the `warn!` from `ignored_state_dir`; callers without user
/// flags (batch, MCP, tests) drop the notes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolverNotes {
    /// The `--state-dir` value that was ignored (`Some` only when
    /// `state_dir.is_some() && !resume`), to be logged by the caller.
    pub ignored_state_dir: Option<PathBuf>,
}

impl PersistenceMode {
    /// Pure resolver — no IO, no logging.
    ///
    /// `default_state_dir` is the caller-supplied fallback (XDG_CACHE_HOME or
    /// `~/.cache/webfang/state`). `state_dir` without `--resume` is ignored;
    /// callers that owe the operator a `warn!` must use
    /// [`from_config_with_notes`](Self::from_config_with_notes) instead.
    pub fn from_config(cfg: &ResumeConfig, default_state_dir: &Path) -> Self {
        Self::from_config_with_notes(cfg, default_state_dir).0
    }

    /// Pure resolver with side-channel notes — no IO, no logging.
    ///
    /// Returns the resolved mode plus [`ResolverNotes`]: `ignored_state_dir`
    /// is `Some` exactly when `--state-dir` was passed without `--resume`
    /// and therefore ignored. The caller that knows about user flags emits
    /// `warn!(state_dir = ?notes.ignored_state_dir, "ignoring --state-dir
    /// without --resume")` from it; all other callers ignore the notes.
    pub fn from_config_with_notes(
        cfg: &ResumeConfig,
        default_state_dir: &Path,
    ) -> (Self, ResolverNotes) {
        let ignored_state_dir = if cfg.state_dir.is_some() && !cfg.resume {
            cfg.state_dir.clone()
        } else {
            None
        };
        let checkpoint_enabled = cfg.checkpoint_interval != 0 && !cfg.no_checkpoint;
        let resume_enabled = cfg.resume;
        let mode = match (resume_enabled, checkpoint_enabled) {
            (false, false) => Self::Disabled,
            (true, false) => {
                let dir = cfg
                    .state_dir
                    .clone()
                    .unwrap_or_else(|| default_state_dir.to_path_buf());
                Self::Resume { dir }
            },
            (false, true) => {
                // --state-dir without --resume is ignored → default dir.
                let checkpoint = CheckpointCfg {
                    dir: default_state_dir.to_path_buf(),
                    interval: cfg.checkpoint_interval,
                };
                Self::Checkpoint { cfg: checkpoint }
            },
            (true, true) => {
                let resume_dir = cfg
                    .state_dir
                    .clone()
                    .unwrap_or_else(|| default_state_dir.to_path_buf());
                let checkpoint = CheckpointCfg {
                    dir: resume_dir.clone(),
                    interval: cfg.checkpoint_interval,
                };
                Self::Full {
                    resume_dir,
                    checkpoint,
                }
            },
        };
        (mode, ResolverNotes { ignored_state_dir })
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

    fn cfg(
        resume: bool,
        state_dir: Option<&str>,
        interval: u64,
        no_checkpoint: bool,
    ) -> ResumeConfig {
        ResumeConfig {
            resume,
            state_dir: state_dir.map(PathBuf::from),
            checkpoint_interval: interval,
            no_checkpoint,
        }
    }

    // ——— 8-combo matrix ———

    #[test]
    fn disabled_when_no_resume_and_checkpoint_disabled_by_zero() {
        let m = PersistenceMode::from_config(&cfg(false, None, 0, false), &default_dir());
        assert_eq!(m, PersistenceMode::Disabled);
    }

    #[test]
    fn disabled_when_no_resume_and_no_checkpoint_flag() {
        let m = PersistenceMode::from_config(&cfg(false, None, 100, true), &default_dir());
        assert_eq!(m, PersistenceMode::Disabled);
    }

    #[test]
    fn resume_only_with_default_dir() {
        let m = PersistenceMode::from_config(&cfg(true, None, 0, false), &default_dir());
        assert_eq!(m, PersistenceMode::Resume { dir: default_dir() });
    }

    #[test]
    fn resume_with_custom_state_dir() {
        let m =
            PersistenceMode::from_config(&cfg(true, Some("/tmp/cache"), 0, false), &default_dir());
        assert_eq!(
            m,
            PersistenceMode::Resume {
                dir: PathBuf::from("/tmp/cache")
            }
        );
    }

    #[test]
    fn checkpoint_only_with_default_dir() {
        let m = PersistenceMode::from_config(&cfg(false, None, 100, false), &default_dir());
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
        let m = PersistenceMode::from_config(&cfg(false, None, 50, false), &default_dir());
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
        let m = PersistenceMode::from_config(&cfg(true, Some("/tmp/x"), 50, false), &default_dir());
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
        let m = PersistenceMode::from_config(&cfg(true, None, 100, false), &default_dir());
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
        let m = PersistenceMode::from_config(&cfg(true, None, 0, false), &default_dir());
        // With interval 0, checkpoint must be disabled → Resume only, not Full.
        assert_eq!(m, PersistenceMode::Resume { dir: default_dir() });
        assert!(m.checkpoint_cfg().is_none());
    }

    #[test]
    fn no_checkpoint_disables_checkpoint() {
        let m = PersistenceMode::from_config(&cfg(true, None, 100, true), &default_dir());
        // no_checkpoint true → Resume only, interval ignored.
        assert_eq!(m, PersistenceMode::Resume { dir: default_dir() });
        assert!(m.checkpoint_cfg().is_none());
    }

    #[test]
    fn checkpoint_disabled_when_interval_zero_even_without_resume() {
        let m = PersistenceMode::from_config(&cfg(false, None, 0, false), &default_dir());
        assert_eq!(m, PersistenceMode::Disabled);
        assert!(m.checkpoint_cfg().is_none());
    }

    // ——— state_dir without --resume is ignored ———

    #[test]
    fn state_dir_without_resume_is_ignored_checkpoint_uses_default() {
        let m = PersistenceMode::from_config(
            &cfg(false, Some("/tmp/cache"), 100, false),
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
        let m =
            PersistenceMode::from_config(&cfg(false, Some("/tmp/cache"), 0, false), &default_dir());
        assert_eq!(m, PersistenceMode::Disabled);
    }

    // ——— ResolverNotes side-channel (#1045) ———

    #[test]
    fn notes_report_ignored_state_dir_only_without_resume() {
        let (_, notes) = PersistenceMode::from_config_with_notes(
            &cfg(false, Some("/tmp/cache"), 100, false),
            &default_dir(),
        );
        assert_eq!(notes.ignored_state_dir, Some(PathBuf::from("/tmp/cache")));
    }

    #[test]
    fn notes_empty_when_resume_consumes_state_dir() {
        let (_, notes) = PersistenceMode::from_config_with_notes(
            &cfg(true, Some("/tmp/cache"), 0, false),
            &default_dir(),
        );
        assert_eq!(notes, ResolverNotes::default());
    }

    #[test]
    fn notes_empty_when_no_state_dir_given() {
        let (_, notes) =
            PersistenceMode::from_config_with_notes(&cfg(false, None, 100, false), &default_dir());
        assert_eq!(notes, ResolverNotes::default());
    }

    #[test]
    fn from_config_agrees_with_notes_variant_on_full_matrix() {
        // `from_config` is the notes-dropping façade: same mode, no notes.
        for (resume, state_dir, interval, no_checkpoint) in [
            (false, None, 0, false),
            (false, Some("/tmp/cache"), 100, false),
            (true, Some("/tmp/cache"), 50, false),
            (true, None, 100, true),
        ] {
            let c = cfg(resume, state_dir, interval, no_checkpoint);
            let (mode, _) = PersistenceMode::from_config_with_notes(&c, &default_dir());
            assert_eq!(PersistenceMode::from_config(&c, &default_dir()), mode);
        }
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

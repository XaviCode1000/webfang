//! THE canonical concurrency clamp.
//!
//! Single-source invariant: every concurrency clamping decision in WebFang
//! goes through [`clamp_budget`] with the shared ceiling
//! [`MAX_CONCURRENCY_CEILING`]. No subsystem may keep an inline
//! `clamp(1, 16)`-style literal; legacy sites delegate here.

use std::num::NonZeroUsize;

/// Process-wide upper ceiling for any auto-derived or operator-supplied
/// concurrency value. Formerly hardcoded as `clamp(1, 16)` literals.
pub(crate) const MAX_CONCURRENCY_CEILING: usize = 16;

/// Clamp a raw concurrency value into a valid budget slot.
///
/// The result is always at least `min` (typically 1, so a zero-semaphore or
/// zero-burst configuration is unrepresentable) and never exceeds `max`.
/// When `max < min`, the floor wins (a NonZero floor can never be violated).
///
/// This is THE one clamp function of the budget model; see the module docs
/// for the single-source invariant.
#[must_use]
pub(crate) fn clamp_budget(value: usize, min: NonZeroUsize, max: usize) -> NonZeroUsize {
    let floored = value.max(min.get());
    // A degenerate `max < min` can never violate the NonZero floor: it wins.
    let capped = floored.min(max.max(min.get()));
    NonZeroUsize::new(capped).unwrap_or(min)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one() -> NonZeroUsize {
        NonZeroUsize::new(1).expect("1 is non-zero")
    }

    #[test]
    fn zero_floors_at_min() {
        assert_eq!(clamp_budget(0, one(), MAX_CONCURRENCY_CEILING).get(), 1);
    }

    #[test]
    fn one_stays_one() {
        assert_eq!(clamp_budget(1, one(), MAX_CONCURRENCY_CEILING).get(), 1);
    }

    #[test]
    fn fifteen_stays() {
        assert_eq!(clamp_budget(15, one(), MAX_CONCURRENCY_CEILING).get(), 15);
    }

    #[test]
    fn ceiling_boundary_sixteen_stays() {
        assert_eq!(clamp_budget(16, one(), MAX_CONCURRENCY_CEILING).get(), 16);
    }

    #[test]
    fn seventeen_caps_at_ceiling() {
        assert_eq!(clamp_budget(17, one(), MAX_CONCURRENCY_CEILING).get(), 16);
    }

    #[test]
    fn usize_max_caps_at_ceiling() {
        assert_eq!(
            clamp_budget(usize::MAX, one(), MAX_CONCURRENCY_CEILING).get(),
            16
        );
    }

    #[test]
    fn custom_floor_wins_over_lower_max() {
        let min = NonZeroUsize::new(4).expect("4 is non-zero");
        assert_eq!(clamp_budget(2, min, 3).get(), 4);
    }

    #[test]
    fn custom_min_honored() {
        let min = NonZeroUsize::new(4).expect("4 is non-zero");
        assert_eq!(clamp_budget(10, min, MAX_CONCURRENCY_CEILING).get(), 10);
    }

    /// Property sweep: every value in 0..=64 lands inside `[min, ceiling]`.
    #[test]
    fn sweep_zero_to_sixty_four_always_in_range() {
        for value in 0..=64usize {
            let clamped = clamp_budget(value, one(), MAX_CONCURRENCY_CEILING);
            assert!(clamped.get() >= 1, "floor violated at {value}");
            assert!(clamped.get() <= 16, "ceiling violated at {value}");
            // Monotonicity: non-decreasing in the input.
            if value > 0 {
                let prev = clamp_budget(value - 1, one(), MAX_CONCURRENCY_CEILING);
                assert!(
                    clamped.get() >= prev.get(),
                    "monotonicity violated at {value}"
                );
            }
        }
    }
}

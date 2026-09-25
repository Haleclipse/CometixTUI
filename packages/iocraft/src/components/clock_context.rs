//! The shared animation clock. Maps to: CC `ink/components/ClockContext.tsx`.
//!
//! CC's `createClock()` is one base `setInterval` (`FRAME_INTERVAL_MS`, twice
//! that while the terminal is blurred) whose subscribers gate on elapsed
//! time; `ClockProvider` owns the instance and Ink's own root `App`
//! (`ink/components/App.tsx:219`) mounts it, so application code never sees
//! the provider and only consumes the clock through hooks. Two things follow
//! from the single instance:
//!
//! - every subscriber reads the same `now()`, so a late-mounted animation
//!   stays in phase with the rest instead of restarting at zero;
//! - equal intervals fire on the same base tick, so they share a frame.
//!
//! Here the render root ([`Tree::render`](crate::render)) provides one
//! [`Clock`] per tree, the way Ink's root mounts `ClockProvider`. iocraft has
//! no base ticker (each hook arms its own `Delay`), so the clock carries the
//! epoch ([`Clock::now_ms`]) and the grid an animation timer arms to
//! ([`Clock::delay_to_next`]) instead of counting from its own start.
//! [`tick_interval`] is `ClockProvider`'s focus policy; with no base tick it
//! is applied per interval, which slows a blurred terminal's animations to
//! half rate rather than CC's base-tick quantization.
//!
//! Consumers: [`use_animation_frame`](crate::hooks::UseAnimationFrame::use_animation_frame),
//! [`use_animation_timer`](crate::hooks::UseInterval::use_animation_timer) and
//! [`use_animation_interval`](crate::hooks::UseAnimationFrame::use_animation_interval).
//! CC's `useInterval` also subscribes to the clock but gates on
//! `now - lastUpdate` from its own subscribe time, so its phase is its
//! start; iocraft's `use_interval` models that with its own `Delay`.

use std::sync::Arc;
use std::time::{Duration, Instant};

/// Handle to a render tree's animation clock. Cheap to clone; every clone
/// reads the same epoch. Obtain it with `hooks.use_context::<Clock>()` (the
/// render root provides it) — an application that batches its own updates
/// can keep a clone and aim at the same grid as the animations.
#[derive(Clone, Debug)]
pub struct Clock {
    inner: Arc<ClockInner>,
}

#[derive(Debug)]
struct ClockInner {
    epoch: Instant,
}

impl Clock {
    /// A fresh clock whose epoch is now. The render root creates one per
    /// tree; tests that drive hooks outside a tree can create their own.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ClockInner {
                epoch: Instant::now(),
            }),
        }
    }

    /// Milliseconds since the clock's epoch. CC `Clock.now()`.
    pub fn now_ms(&self) -> u128 {
        self.inner.epoch.elapsed().as_millis()
    }

    /// Time until the clock next reaches a multiple of `interval`: the grid
    /// an animation timer arms to, so every timer of that interval fires at
    /// the same instant. Strictly positive and at most `interval`.
    pub fn delay_to_next(&self, interval: Duration) -> Duration {
        let interval_ms = interval.as_millis().max(1);
        let now = self.now_ms();
        let next = (now / interval_ms + 1) * interval_ms;
        Duration::from_millis((next - now) as u64)
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}

/// `ClockProvider`'s focus policy: `FRAME_INTERVAL_MS` while the terminal is
/// focused, `BLURRED_TICK_INTERVAL_MS = FRAME_INTERVAL_MS * 2` while it is
/// not. Applied per animation interval, since there is no base tick to slow
/// down: an unfocused terminal animates at half rate.
pub fn tick_interval(interval: Duration, focused: bool) -> Duration {
    if focused {
        interval
    } else {
        interval.saturating_mul(2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_to_next_lands_on_the_next_grid_line() {
        let clock = Clock::new();
        let interval = Duration::from_millis(100);
        let before = clock.now_ms();
        let delay = clock.delay_to_next(interval);
        // Strictly in (0, interval]: never zero (that would re-fire the same
        // line), never past the next line.
        assert!(delay > Duration::ZERO && delay <= interval, "{delay:?}");
        let target = before + delay.as_millis();
        assert_eq!(target % 100, 0, "target {target} is not on the 100ms grid");
    }

    #[test]
    fn clones_share_the_epoch() {
        let clock = Clock::new();
        let other = clock.clone();
        std::thread::sleep(Duration::from_millis(2));
        assert!(other.now_ms() >= 2);
        assert!(clock.now_ms().abs_diff(other.now_ms()) <= 1);
    }

    #[test]
    fn blurred_terminal_animates_at_half_rate() {
        let interval = Duration::from_millis(50);
        assert_eq!(tick_interval(interval, true), interval);
        assert_eq!(tick_interval(interval, false), Duration::from_millis(100));
    }
}

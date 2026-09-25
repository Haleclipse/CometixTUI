//! The shared animation clock. Maps to: CC `ink/components/ClockContext.tsx`.
//!
//! CC's `createClock()` is one base `setInterval` (`FRAME_INTERVAL_MS`, twice
//! that while the terminal is blurred) whose subscribers gate on elapsed
//! time; `ClockProvider` owns the instance and Ink's own root `App`
//! (`ink/components/App.tsx:219`) mounts it, so application code never sees
//! the provider and only consumes the clock through hooks. Three things
//! follow from the single base tick:
//!
//! - every subscriber reads the same `now()`, so a late-mounted animation
//!   stays in phase with the rest instead of restarting at zero;
//! - equal intervals fire on the same base tick, so they share a frame;
//! - an interval is quantized to the base tick: a subscriber asking for 50ms
//!   fires on the first tick at which 50ms have elapsed, which is the 64ms
//!   tick, and one asking for 100ms fires every 112ms. (CC 2.1.88's
//!   `useInterval` gates on `now - lastUpdate >= intervalMs`; later versions
//!   write the same rule as `ceil(interval / FRAME_INTERVAL_MS) *
//!   FRAME_INTERVAL_MS`.) `ClockProvider` doubles the base tick while the
//!   terminal is blurred, so every animation slows without any consumer
//!   reading the focus state — `useAnimationFrame` documents exactly that.
//!
//! Here the render root ([`Tree::render`](crate::render)) provides one
//! [`Clock`] per tree, the way Ink's root mounts `ClockProvider`, and sets
//! its base tick from the terminal's focus state on each render, as
//! `ClockProvider` does from `useTerminalFocus`. iocraft has no base ticker
//! (each hook arms its own `Delay`), so the clock carries the epoch
//! ([`Clock::now_ms`]), the quantization ([`Clock::period`]) and the grid an
//! animation timer arms to ([`Clock::delay_to_next`]) instead of counting
//! from its own start.
//!
//! Consumers: [`use_animation_frame`](crate::hooks::UseAnimationFrame::use_animation_frame),
//! [`use_animation_timer`](crate::hooks::UseInterval::use_animation_timer) and
//! [`use_animation_interval`](crate::hooks::UseAnimationFrame::use_animation_interval).
//! CC's `useInterval` also subscribes to the clock but gates on
//! `now - lastUpdate` from its own subscribe time, so its phase is its
//! start; iocraft's `use_interval` models that with its own `Delay`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The base tick while the terminal is focused. CC `ink/constants.ts`
/// `FRAME_INTERVAL_MS`.
pub const FRAME_INTERVAL_MS: u64 = 16;

/// The base tick while the terminal is blurred. CC `ClockContext.tsx`
/// `BLURRED_TICK_INTERVAL_MS = FRAME_INTERVAL_MS * 2`.
pub const BLURRED_TICK_INTERVAL_MS: u64 = FRAME_INTERVAL_MS * 2;

/// Handle to a render tree's animation clock. Cheap to clone; every clone
/// reads the same epoch and base tick. Obtain it with
/// `hooks.use_context::<Clock>()` (the render root provides it) — an
/// application that batches its own updates can keep a clone and aim at the
/// same grid as the animations.
#[derive(Clone, Debug)]
pub struct Clock {
    inner: Arc<ClockInner>,
}

#[derive(Debug)]
struct ClockInner {
    epoch: Instant,
    tick_interval_ms: AtomicU64,
}

impl Clock {
    /// A fresh clock whose epoch is now and whose base tick is
    /// [`FRAME_INTERVAL_MS`]. The render root creates one per tree; tests
    /// that drive hooks outside a tree can create their own.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ClockInner {
                epoch: Instant::now(),
                tick_interval_ms: AtomicU64::new(FRAME_INTERVAL_MS),
            }),
        }
    }

    /// Milliseconds since the clock's epoch. CC `Clock.now()`.
    pub fn now_ms(&self) -> u128 {
        self.inner.epoch.elapsed().as_millis()
    }

    /// The base tick every interval is quantized to. CC
    /// `Clock.setTickInterval`; the render root calls it with
    /// [`tick_interval_for_focus`] on each render.
    pub fn set_tick_interval(&self, tick: Duration) {
        let ms = u64::try_from(tick.as_millis()).unwrap_or(u64::MAX).max(1);
        self.inner.tick_interval_ms.store(ms, Ordering::Relaxed);
    }

    /// The current base tick.
    pub fn tick_interval(&self) -> Duration {
        Duration::from_millis(self.inner.tick_interval_ms.load(Ordering::Relaxed))
    }

    /// The period an animation asking for `interval` actually gets: the
    /// smallest multiple of the base tick that is at least `interval`. With
    /// the focused 16ms tick, 50ms → 64ms, 100ms → 112ms, 120ms → 128ms.
    pub fn period(&self, interval: Duration) -> Duration {
        let tick = self.inner.tick_interval_ms.load(Ordering::Relaxed) as u128;
        let interval_ms = interval.as_millis().max(1);
        let period = interval_ms.div_ceil(tick) * tick;
        Duration::from_millis(u64::try_from(period).unwrap_or(u64::MAX))
    }

    /// Time until the clock next reaches a multiple of
    /// [`period`](Self::period)`(interval)`: the grid an animation timer arms
    /// to, so every timer of that interval fires at the same instant.
    /// Strictly positive and at most the period.
    pub fn delay_to_next(&self, interval: Duration) -> Duration {
        let period_ms = self.period(interval).as_millis().max(1);
        let now = self.now_ms();
        let next = (now / period_ms + 1) * period_ms;
        Duration::from_millis(u64::try_from(next - now).unwrap_or(u64::MAX))
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}

/// `ClockProvider`'s focus policy: [`FRAME_INTERVAL_MS`] while the terminal
/// is focused (or its focus state is unknown), [`BLURRED_TICK_INTERVAL_MS`]
/// while it is not. Every animation then runs on the coarser grid.
pub fn tick_interval_for_focus(focused: bool) -> Duration {
    Duration::from_millis(if focused {
        FRAME_INTERVAL_MS
    } else {
        BLURRED_TICK_INTERVAL_MS
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_is_the_next_multiple_of_the_base_tick() {
        let clock = Clock::new();
        let period = |ms: u64| clock.period(Duration::from_millis(ms)).as_millis();
        assert_eq!(period(16), 16);
        assert_eq!(period(1), 16);
        assert_eq!(period(50), 64);
        assert_eq!(period(100), 112);
        assert_eq!(period(120), 128);
        assert_eq!(period(1000), 1008);
    }

    #[test]
    fn blurred_terminal_quantizes_to_the_doubled_tick() {
        let clock = Clock::new();
        clock.set_tick_interval(tick_interval_for_focus(false));
        assert_eq!(clock.tick_interval(), Duration::from_millis(32));
        let period = |ms: u64| clock.period(Duration::from_millis(ms)).as_millis();
        assert_eq!(period(50), 64);
        assert_eq!(period(100), 128);
        assert_eq!(period(120), 128);
        clock.set_tick_interval(tick_interval_for_focus(true));
        assert_eq!(clock.tick_interval(), Duration::from_millis(16));
        assert_eq!(period(100), 112);
    }

    #[test]
    fn delay_to_next_lands_on_the_period_grid() {
        let clock = Clock::new();
        let interval = Duration::from_millis(100);
        let period = clock.period(interval).as_millis();
        let before = clock.now_ms();
        let delay = clock.delay_to_next(interval);
        // Strictly in (0, period]: never zero (that would re-fire the same
        // line), never past the next line.
        assert!(
            delay > Duration::ZERO && delay.as_millis() <= period,
            "{delay:?}"
        );
        let target = before + delay.as_millis();
        assert_eq!(
            target % period,
            0,
            "target {target} is not on the {period}ms grid"
        );
    }

    #[test]
    fn clones_share_the_epoch_and_the_tick() {
        let clock = Clock::new();
        let other = clock.clone();
        std::thread::sleep(Duration::from_millis(2));
        assert!(other.now_ms() >= 2);
        assert!(clock.now_ms().abs_diff(other.now_ms()) <= 1);
        other.set_tick_interval(Duration::from_millis(32));
        assert_eq!(clock.tick_interval(), Duration::from_millis(32));
    }
}

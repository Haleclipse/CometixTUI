use crate::{Hook, Hooks};
use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use futures_timer::Delay;
use std::{
    sync::OnceLock,
    time::{Duration, Instant},
};

use super::UseState;

mod private {
    pub trait Sealed {}
    impl Sealed for crate::Hooks<'_, '_> {}
}

static SHARED_CLOCK_START: OnceLock<Instant> = OnceLock::new();

pub(crate) fn shared_clock_now_ms() -> u128 {
    SHARED_CLOCK_START
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
}

/// Interval and animation-timer hooks.
///
/// These mirror the CC Ink fork's `useInterval(...)` / `useAnimationTimer(...)`
/// shape. Timers are driven by iocraft's hook polling, while animation time is
/// read from a process-wide shared clock so late-mounted timers stay in sync
/// instead of restarting at zero. Passing `None` pauses the interval without
/// clearing the last observed timer value.
pub trait UseInterval: private::Sealed {
    /// Calls `callback` every `interval` while the component is mounted.
    ///
    /// Passing `None` pauses the interval. The callback is refreshed on every
    /// render, so it can capture the latest props/state while keeping the hook
    /// slot stable.
    fn use_interval<F>(&mut self, callback: F, interval: Option<Duration>)
    where
        F: FnMut() + Send + Unpin + 'static;

    /// Returns milliseconds elapsed on the shared animation clock, updated no
    /// more often than `interval`.
    fn use_animation_timer(&mut self, interval: Duration) -> u128 {
        self.use_animation_timer_opt(Some(interval))
    }

    /// Pausable variant of [`UseInterval::use_animation_timer`].
    fn use_animation_timer_opt(&mut self, interval: Option<Duration>) -> u128;
}

impl UseInterval for Hooks<'_, '_> {
    fn use_interval<F>(&mut self, callback: F, interval: Option<Duration>)
    where
        F: FnMut() + Send + Unpin + 'static,
    {
        let hook = self.use_hook(UseIntervalImpl::<F>::default);
        if hook.interval != interval {
            hook.delay = None;
        }
        hook.interval = interval;
        hook.callback = Some(callback);
        // The `Delay` is created and its waker armed in `poll_change`. A
        // render that (re)starts the interval has to be followed by one poll,
        // or a push-mode component nobody else wakes never sees its timer.
        let needs_arm = interval.is_some() && hook.delay.is_none();
        if needs_arm {
            self.request_poll();
        }
    }

    fn use_animation_timer_opt(&mut self, interval: Option<Duration>) -> u128 {
        let now = self.use_state(|| {
            if interval.is_some() {
                shared_clock_now_ms()
            } else {
                0
            }
        });
        let mut now_for_callback = now;
        self.use_interval(
            move || {
                now_for_callback.set(shared_clock_now_ms());
            },
            interval,
        );
        now.get()
    }
}

struct UseIntervalImpl<F> {
    callback: Option<F>,
    interval: Option<Duration>,
    delay: Option<Pin<Box<Delay>>>,
}

impl<F> Default for UseIntervalImpl<F> {
    fn default() -> Self {
        Self {
            callback: None,
            interval: None,
            delay: None,
        }
    }
}

impl<F> Hook for UseIntervalImpl<F>
where
    F: FnMut() + Send + Unpin,
{
    fn poll_change(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let Some(interval) = self.interval else {
            self.delay = None;
            return Poll::Pending;
        };

        if self.delay.is_none() {
            self.delay = Some(Box::pin(Delay::new(interval)));
        }

        // A tick is not a visual change: run the callback, re-arm, and stay
        // Pending. Anything the callback wants on screen goes through a state
        // hook, whose own poll_change reports the render — matching React/Ink,
        // where a setInterval callback does not render and only setState does.
        // Returning Ready here instead forced a full render per tick, which on
        // a large canvas burned ~30% CPU at idle from no-op polls alone.
        //
        // The loop re-polls the fresh Delay so it registers its waker with
        // `cx`; without that the interval would never fire again.
        loop {
            let ready = self
                .delay
                .as_mut()
                .is_some_and(|delay| delay.as_mut().poll(cx).is_ready());
            if !ready {
                return Poll::Pending;
            }

            self.delay = Some(Box::pin(Delay::new(interval)));
            if let Some(callback) = self.callback.as_mut() {
                callback();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;
    use futures::StreamExt;

    #[component]
    fn IntervalCounter(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
        let mut system = hooks.use_context_mut::<SystemContext>();
        let count = hooks.use_state(|| 0u8);
        let count_for_callback = count;
        hooks.use_interval(
            move || {
                let mut count = count_for_callback;
                count += 1;
            },
            Some(Duration::from_millis(1)),
        );

        if count.get() >= 2 {
            system.exit();
        }

        element!(Text(content: format!("count={}", count.get())))
    }

    #[test]
    fn test_use_interval_ticks_until_paused_by_exit() {
        let canvases: Vec<_> = smol::block_on(
            element!(IntervalCounter)
                .mock_terminal_render_loop(MockTerminalConfig::default())
                .collect(),
        );
        assert_eq!(canvases.last().unwrap().to_string(), "count=2\n");
    }

    #[component]
    fn PausedTimer(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
        let mut system = hooks.use_context_mut::<SystemContext>();
        let timer = hooks.use_animation_timer_opt(None);
        system.exit();
        element!(Text(content: format!("timer={timer}")))
    }

    #[test]
    fn test_use_animation_timer_none_is_paused() {
        let canvases: Vec<_> = smol::block_on(
            element!(PausedTimer)
                .mock_terminal_render_loop(MockTerminalConfig::default())
                .collect(),
        );
        assert_eq!(canvases.last().unwrap().to_string(), "timer=0\n");
    }

    #[component]
    fn LateTimerChild(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
        let mut system = hooks.use_context_mut::<SystemContext>();
        let timer = hooks.use_animation_timer(Duration::from_millis(1));
        system.exit();
        element!(Text(content: format!("late={timer}")))
    }

    #[component]
    fn SharedTimerApp(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
        let timer = hooks.use_animation_timer(Duration::from_millis(1));
        element! {
            View(flex_direction: FlexDirection::Column) {
                Text(content: format!("parent={timer}"))
                #(if timer > 0 {
                    element!(LateTimerChild).into_any()
                } else {
                    element!(Text(content: "waiting")).into_any()
                })
            }
        }
    }

    #[test]
    fn test_use_animation_timer_uses_shared_clock_for_late_mounts() {
        let canvases: Vec<_> = smol::block_on(
            element!(SharedTimerApp)
                .mock_terminal_render_loop(MockTerminalConfig::default())
                .collect(),
        );
        let rendered = canvases.last().unwrap().to_string();
        let late = rendered
            .lines()
            .find_map(|line| line.strip_prefix("late="))
            .and_then(|value| value.parse::<u128>().ok())
            .expect("late-mounted timer should render its shared clock time");
        assert!(
            late > 0,
            "late-mounted animation timers should inherit the shared clock instead of restarting at 0: {rendered:?}"
        );
    }

    // An interval that is paused on the first render and started by a later
    // one. A component's first poll arms whatever its hooks wait for; a hook
    // that only begins waiting after a later render has to ask for that poll
    // itself (`Hooks::request_poll`), or under push-mode wake routing nothing
    // polls it and the timer never fires (the ScrollView drain hang).
    #[component]
    fn LateStartInterval(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
        let mut system = hooks.use_context_mut::<SystemContext>();
        let armed = hooks.use_state(|| false);
        let count = hooks.use_state(|| 0u8);
        let mut armed_for_future = armed;
        hooks.use_future(async move {
            armed_for_future.set(true);
        });
        let mut count_for_interval = count;
        hooks.use_interval(
            move || count_for_interval += 1,
            armed.get().then_some(Duration::from_millis(1)),
        );
        if count.get() >= 2 {
            system.exit();
        }
        element!(Text(content: format!("armed={} count={}", armed.get(), count.get())))
    }

    #[test]
    fn test_interval_started_by_a_later_render_fires_under_push_wake() {
        use futures::FutureExt;
        crate::render::wake::set_push_wake_for_tests(Some(true));
        let mut app = element!(LateStartInterval);
        let frames = smol::block_on(futures::future::select(
            app.mock_terminal_render_loop(MockTerminalConfig::default())
                .map(|canvas| canvas.to_string())
                .collect::<Vec<_>>()
                .boxed_local(),
            smol::Timer::after(Duration::from_secs(5)),
        ));
        crate::render::wake::set_push_wake_for_tests(None);
        let frames = match frames {
            futures::future::Either::Left((frames, _)) => frames,
            futures::future::Either::Right(_) => {
                panic!("render loop parked: an interval started by a later render was never polled")
            }
        };
        assert_eq!(frames.last().unwrap(), "armed=true count=2\n");
    }
}

//! Push-mode wake routing for the component tree (dirty-path rendering, Phase W).
//!
//! In pull mode (the default), every frame re-polls every component's hooks so
//! each hook re-registers the render loop's waker — O(tree) work per frame even
//! when nothing changed. In push mode (`IOCRAFT_PUSH_WAKE=1`), each component
//! polls its own hooks through a per-component proxy waker: a wake marks that
//! component dirty *and* wakes the render loop, so the next frame's poll pass
//! can skip every non-dirty component with a single atomic read — their hook
//! wakers are still armed from the last poll, and the `Future` contract
//! guarantees any change will fire them.
//!
//! Design doc: CometixCode `docs/IOCRAFT_DIRTY_PATH_RENDER_DESIGN_2026-09-23.md`
//! (Phase W). The React analogue is `setState` scheduling work on the fiber
//! instead of the reconciler polling every component for changes.

use futures::task::AtomicWaker;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::task::Wake;

/// Opt-in gate for push-mode wake routing. Off by default while the harvest
/// scan is validated A/B against the full-tree poll.
pub(crate) fn push_wake_enabled() -> bool {
    #[cfg(test)]
    {
        match PUSH_WAKE_TEST_OVERRIDE.with(|cell| cell.get()) {
            1 => return false,
            2 => return true,
            _ => {}
        }
    }
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("IOCRAFT_PUSH_WAKE")
            .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    })
}

/// 0 = no override, 1 = forced off, 2 = forced on. Thread-local (not a
/// process-wide atomic): the mock render loop runs on the test's own thread,
/// so a thread-local override flips the gate for exactly that test while
/// parallel frame-cadence-sensitive tests keep the default behavior.
#[cfg(test)]
thread_local! {
    pub(crate) static PUSH_WAKE_TEST_OVERRIDE: std::cell::Cell<u8> =
        const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn set_push_wake_for_tests(enabled: Option<bool>) {
    PUSH_WAKE_TEST_OVERRIDE.with(|cell| {
        cell.set(match enabled {
            None => 0,
            Some(false) => 1,
            Some(true) => 2,
        })
    });
}

/// The render loop's waker, shared by every component in one tree. The loop
/// re-registers its current waker at the top of every root poll; component
/// proxies wake through here so a state change anywhere still drives exactly
/// one loop wakeup.
#[derive(Default)]
pub(crate) struct RootWakeSlot {
    pub(crate) waker: AtomicWaker,
}

/// Per-component wake state: the dirty bit consumed by the harvest scan plus
/// the shared route back to the render loop.
///
/// Held as an `Arc` by both the component and any waker clones handed to
/// hooks/futures, so a wake that arrives after the component was dropped only
/// flips an orphaned bit — it never dereferences component memory.
pub(crate) struct ComponentWakeState {
    dirty: AtomicBool,
    root: Arc<RootWakeSlot>,
}

impl ComponentWakeState {
    /// New components start dirty so their first poll arms every hook waker.
    pub(crate) fn new(root: Arc<RootWakeSlot>) -> Self {
        Self {
            dirty: AtomicBool::new(true),
            root,
        }
    }

    pub(crate) fn root(&self) -> &Arc<RootWakeSlot> {
        &self.root
    }

    /// Consumes the dirty bit for this frame's harvest decision.
    pub(crate) fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::AcqRel)
    }

    /// Reads the dirty bit without consuming it. The settle pass only peeks:
    /// consuming here would swallow a wake that also belongs to a future or
    /// event hook on the same component, which the next frame's harvest must
    /// still poll.
    pub(crate) fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Acquire)
    }

    /// Re-marks the component dirty without waking the loop (used when a poll
    /// must be retried next frame, e.g. a borrow conflict).
    pub(crate) fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Release);
    }
}

impl Wake for ComponentWakeState {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.dirty.store(true, Ordering::Release);
        self.root.waker.wake();
    }
}

#[cfg(test)]
mod tests {
    use crate::prelude::*;
    use futures::stream::StreamExt;
    use macro_rules_attribute::apply;
    use smol_macros::test;
    use std::time::Duration;

    // A deep child whose only change source is its own state, driven by a
    // future — the exact shape the harvest scan must keep alive: the parent
    // never re-renders it, so its wake must land on its own dirty bit.
    //
    // The deep counter itself ends the run once it has advanced: a parent
    // counting its own frames is not a clock (settle rounds legitimately fold
    // several update-phase bumps into one frame).
    #[component]
    fn DeepCounter(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
        let mut system = hooks.use_context_mut::<SystemContext>();
        let mut count = hooks.use_state(|| 0u32);
        hooks.use_future(async move {
            loop {
                smol::Timer::after(Duration::from_millis(5)).await;
                count += 1;
            }
        });
        if count.get() >= 3 {
            system.exit();
        }
        element!(Text(content: format!("count={}", count)))
    }

    #[component]
    fn HarvestApp() -> impl Into<AnyElement<'static>> {
        element! {
            View(flex_direction: FlexDirection::Column) {
                Text(content: "static header")
                View {
                    View {
                        DeepCounter
                    }
                }
            }
        }
    }

    #[apply(test!)]
    async fn test_push_wake_deep_state_change_still_renders() {
        super::set_push_wake_for_tests(Some(true));
        let frames = element!(HarvestApp)
            .mock_terminal_render_loop(MockTerminalConfig::default())
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .await;
        super::set_push_wake_for_tests(None);
        let last = frames.last().expect("at least one frame");
        // The deep counter advanced past its initial value: its future's
        // wakes were routed and harvested rather than dropped.
        assert!(
            last.contains("count=3"),
            "deep counter never advanced under push-wake harvesting: {last:?}"
        );
        assert!(last.contains("static header"));
    }

    #[component]
    fn RectProbe(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
        let mut system = hooks.use_context_mut::<SystemContext>();
        let rect = hooks.use_component_rect();
        let Some(rect) = rect else {
            return element! { Text(content: "pending") };
        };
        system.exit();
        element! {
            Text(content: format!("{}:{}", rect.left, rect.top))
        }
    }

    #[apply(test!)]
    async fn test_push_wake_component_rect_second_pass() {
        super::set_push_wake_for_tests(Some(true));
        let frames = element!(
            View(width: 20, height: 5, justify_content: JustifyContent::CENTER) { RectProbe }
        )
        .mock_terminal_render_loop(MockTerminalConfig::default())
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .await;
        super::set_push_wake_for_tests(None);
        let last = frames.last().expect("at least one frame");
        // The draw-phase rect change must wake its component: without the
        // stored waker the app would hang on "pending" forever (the harvest
        // scan no longer polls clean components each frame).
        assert!(
            !last.contains("pending"),
            "use_component_rect never delivered under push-wake: {last:?}"
        );
    }
}

#[cfg(test)]
mod settle_tests {
    use crate::prelude::*;
    use futures::stream::StreamExt;
    use macro_rules_attribute::apply;
    use smol_macros::test;
    use std::time::Duration;

    // The update body derives `mirror` from `source` — the exact
    // update-phase-write pattern that used to cost one extra "convergence"
    // frame per source change. With settling, each source change must render
    // exactly one frame that already shows the settled mirror value.
    #[component]
    fn SettleApp(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
        let mut system = hooks.use_context_mut::<SystemContext>();
        let source = hooks.use_state(|| 0u32);
        let mut mirror = hooks.use_state(|| 0u32);
        let mut source_driver = source;
        hooks.use_future(async move {
            loop {
                smol::Timer::after(Duration::from_millis(20)).await;
                source_driver += 1;
            }
        });
        if mirror.get() != source.get() {
            mirror.set(source.get());
        }
        if source.get() >= 3 {
            system.exit();
        }
        element!(Text(content: format!("s={} m={}", source, mirror)))
    }

    #[apply(test!)]
    async fn test_update_phase_writes_settle_in_same_frame() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        super::set_push_wake_for_tests(Some(true));
        // Count render-loop ITERATIONS via the profile callback: convergence
        // frames write nothing (the canvas is unchanged), so the output
        // stream cannot see them — only the per-iteration profile can.
        let iterations = Arc::new(AtomicUsize::new(0));
        let iterations_cb = Arc::clone(&iterations);
        let frames = element!(SettleApp)
            .mock_terminal_render_loop_with_profile(MockTerminalConfig::default(), move |_| {
                iterations_cb.fetch_add(1, Ordering::SeqCst);
            })
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .await;
        super::set_push_wake_for_tests(None);
        // Every rendered frame must already show mirror == source.
        for frame in &frames {
            let s = frame.split("s=").nth(1).and_then(|r| r.split(' ').next());
            let m = frame.split("m=").nth(1).map(|r| r.trim());
            assert_eq!(s, m, "frame rendered before settling: {frame:?}");
        }
        // One loop iteration per source value (initial + 3 bumps), with no
        // convergence iterations: the update-phase `mirror.set` was consumed
        // by the settle pass instead of waking an extra empty iteration.
        let n = iterations.load(Ordering::SeqCst);
        assert!(
            n <= 5,
            "expected settled iterations, got {n} loop iterations"
        );
    }

    // Self-driving update-phase counter: every update bumps its own state
    // until it exits. Under push-wake + settle, the second bump lands after a
    // settle pass already consumed the first — it must still find an armed
    // waker, or the component's dirty bit is never set and the loop parks
    // forever (the W1.5 deadlock seen in five CometixCode tests).
    #[component]
    fn SelfDrivingApp(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
        let mut system = hooks.use_context_mut::<SystemContext>();
        let mut tick = hooks.use_state(|| 0u8);
        if tick.get() < 4 {
            tick += 1;
        } else {
            system.exit();
        }
        element!(Text(content: format!("tick {}", tick)))
    }

    #[apply(test!)]
    async fn test_repeated_update_phase_writes_do_not_strand_under_push_wake() {
        use futures::FutureExt;
        super::set_push_wake_for_tests(Some(true));
        let mut app = element!(SelfDrivingApp);
        let frames = futures::future::select(
            app.mock_terminal_render_loop(MockTerminalConfig::default())
                .map(|c| c.to_string())
                .collect::<Vec<_>>()
                .boxed_local(),
            smol::Timer::after(Duration::from_secs(5)),
        )
        .await;
        super::set_push_wake_for_tests(None);
        let frames = match frames {
            futures::future::Either::Left((frames, _)) => frames,
            futures::future::Either::Right(_) => {
                panic!("render loop parked: an update-phase write found no armed waker")
            }
        };
        assert!(frames.last().unwrap().contains("tick 4"));
    }

    // A future that would resolve immediately if polled. The settle pass
    // absorbs render-phase *state writes* only; driving futures inside the
    // render phase would pull async work (file reads, tokio I/O) into a
    // one-shot render and collapse every "pending" first frame.
    #[component]
    fn EagerFutureProbe(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
        let mut resolved = hooks.use_state(|| false);
        let mut renders = hooks.use_state(|| 0u8);
        // A render-phase write, so the settle pass definitely runs.
        if renders.get() == 0 {
            renders.set(1);
        }
        hooks.use_future(async move {
            resolved.set(true);
        });
        element!(Text(content: format!("resolved={}", resolved)))
    }

    #[test]
    fn test_settle_does_not_drive_futures() {
        super::set_push_wake_for_tests(Some(true));
        let rendered = element!(EagerFutureProbe).render(Some(40)).to_string();
        super::set_push_wake_for_tests(None);
        assert!(
            rendered.contains("resolved=false"),
            "settle polled a future inside the render phase: {rendered:?}"
        );
    }
}

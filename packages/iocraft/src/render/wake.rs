//! Push-mode wake routing for the component tree (dirty-path rendering, Phase W).
//!
//! In pull mode (`IOCRAFT_DISABLE=push-wake`), every frame re-polls every component's
//! hooks so each hook re-registers the render loop's waker — O(tree) work per
//! frame even when nothing changed. In push mode (the default), each component
//! polls its own hooks through a per-component proxy waker: a wake marks that
//! component dirty *and* wakes the render loop, so the next frame's poll pass
//! can skip every non-dirty component with a single atomic read — their hook
//! wakers are still armed from the last poll, and the `Future` contract
//! guarantees any change will fire them. A hook whose interest set changes
//! during render (an interval starting, a period changing) asks for one more
//! poll through [`crate::Hooks::request_poll`], which is the same contract
//! applied to render-time reconfiguration.
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

/// Gate for push-mode wake routing. On by default; `IOCRAFT_DISABLE=push-wake`
/// restores pull mode as a kill-switch. Both this crate's suite and
/// CometixCode's pass under either mode: the tests that had depended on pull
/// mode's frame cadence (a `State` bumped during render committing as its own
/// frame, a passive component being polled every frame) were rewritten to be
/// wake-mode independent before the default flipped.
pub(crate) fn push_wake_enabled() -> bool {
    #[cfg(test)]
    {
        match PUSH_WAKE_TEST_OVERRIDE.with(|cell| cell.get()) {
            1 => return false,
            2 => return true,
            _ => {}
        }
    }
    !crate::debug_env::disabled().push_wake
}

// 0 = no override, 1 = forced off, 2 = forced on. Thread-local (not a
// process-wide atomic): the mock render loop runs on the test's own thread,
// so a thread-local override flips the gate for exactly that test while
// parallel frame-cadence-sensitive tests keep the default behavior.
#[cfg(test)]
thread_local! {
    pub(crate) static PUSH_WAKE_TEST_OVERRIDE: std::cell::Cell<u8> =
        const { std::cell::Cell::new(0) };
}

/// `IOCRAFT_DEBUG=settle`: print the type name of every component whose
/// render-phase state write makes the settle pass re-run the update. Each
/// line is one extra update per frame for that component's subtree.
pub(crate) fn settle_trace_enabled() -> bool {
    crate::debug_env::diagnostics().settle
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

/// Per-component wake state: the dirty bit consumed by the harvest scan, the
/// subtree bit that lets the scan skip clean subtrees, the parent link the
/// subtree bit is propagated along, and the shared route back to the render
/// loop.
///
/// Held as an `Arc` by both the component and any waker clones handed to
/// hooks/futures, so a wake that arrives after the component was dropped only
/// flips orphaned bits — it never dereferences component memory. Parent links
/// point up only, so there is no cycle: a dropped subtree releases its states.
pub(crate) struct ComponentWakeState {
    dirty: AtomicBool,
    // Self or some descendant is dirty (dirty-path W2). Set bottom-up by every
    // wake, consumed top-down by the harvest scan, which returns early from any
    // component whose bit is clear instead of visiting every descendant to
    // read its dirty bit — on a large tree that visit was the single biggest
    // per-keystroke cost, more than the update pass itself.
    subtree_dirty: AtomicBool,
    parent: Option<Arc<ComponentWakeState>>,
    root: Arc<RootWakeSlot>,
}

impl ComponentWakeState {
    /// The root of a tree. New components start dirty so their first poll arms
    /// every hook waker.
    pub(crate) fn new_root(root: Arc<RootWakeSlot>) -> Arc<Self> {
        Arc::new(Self {
            dirty: AtomicBool::new(true),
            subtree_dirty: AtomicBool::new(true),
            parent: None,
            root,
        })
    }

    /// A component instantiated under `parent`. Marks the ancestors as well:
    /// the scan after the render that mounted it only descends dirty paths,
    /// and this is how the new component's first poll becomes reachable.
    pub(crate) fn new_child(parent: &Arc<ComponentWakeState>) -> Arc<Self> {
        let state = Arc::new(Self {
            dirty: AtomicBool::new(true),
            subtree_dirty: AtomicBool::new(true),
            parent: Some(Arc::clone(parent)),
            root: Arc::clone(&parent.root),
        });
        parent.mark_subtree_dirty_upwards();
        state
    }

    pub(crate) fn root(&self) -> &Arc<RootWakeSlot> {
        &self.root
    }

    /// Sets the subtree bit here and on every ancestor. Stops at the first one
    /// already set: everything above it is set too, or a scan is currently
    /// between consuming that ancestor's bit and descending to here — either
    /// way this path is still reached. The loop wake is the caller's job.
    fn mark_subtree_dirty_upwards(&self) {
        let mut node = Some(self);
        while let Some(state) = node {
            if state.subtree_dirty.swap(true, Ordering::AcqRel) {
                break;
            }
            node = state.parent.as_deref();
        }
    }

    /// Consumes the dirty bit for this frame's harvest decision.
    pub(crate) fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::AcqRel)
    }

    /// Consumes the subtree bit: `false` means no wake landed on this
    /// component or below since the last scan, so the scan can skip the whole
    /// subtree. Consumed before `take_dirty` because a wake sets `dirty` first
    /// and then marks the path, so a wake racing the scan is never lost: the
    /// path it marks afterwards brings the next scan back here.
    pub(crate) fn take_subtree_dirty(&self) -> bool {
        self.subtree_dirty.swap(false, Ordering::AcqRel)
    }

    /// Reads the dirty bit without consuming it. The settle pass only peeks:
    /// consuming here would swallow a wake that also belongs to a future or
    /// event hook on the same component, which the next frame's harvest must
    /// still poll.
    pub(crate) fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Acquire)
    }

    /// Reads the subtree bit without consuming it (settle pass).
    pub(crate) fn is_subtree_dirty(&self) -> bool {
        self.subtree_dirty.load(Ordering::Acquire)
    }
}

impl Wake for ComponentWakeState {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.dirty.store(true, Ordering::Release);
        self.mark_subtree_dirty_upwards();
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

    #[derive(Default, Props)]
    struct EchoChildProps {
        on_key: Handler<u8>,
    }

    // A key handler that writes its own state — declared before the event
    // hook, so already polled this pass — and, through a handler, its
    // parent's state, which is polled after the child. The pass reports
    // Ready via the parent; the child's write is stranded unless the loop
    // drains wakes before rendering, and would then be consumed by the settle
    // pass as if it were a render-phase write.
    #[component]
    fn EchoChild(mut hooks: Hooks, props: &EchoChildProps) -> impl Into<AnyElement<'static>> {
        let own = hooks.use_state(|| 0u8);
        let on_key = props.on_key.clone();
        hooks.use_terminal_events(move |event| {
            if let TerminalEvent::Key(_) = event {
                let mut own = own;
                own.set(own.get() + 1);
                on_key(own.get());
            }
        });
        element!(Text(content: format!("own={}", own.get())))
    }

    #[component]
    fn EchoParentApp(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
        let mut system = hooks.use_context_mut::<SystemContext>();
        // The parent listens to keys too (as an app shell does), so the key
        // wakes it as well: dirty at pass entry, its hooks are polled after
        // the child's and see the child's write to `mirrored` — the pass
        // reports Ready without ever re-polling the child's own state.
        hooks.use_terminal_events(|_| {});
        let mirrored = hooks.use_state(|| 0u8);
        let mirrored_for_child = mirrored;
        // Exit from a timer, not from the key frames: an exit during update
        // ends the frame early, and the key frames are the ones under test.
        let done = hooks.use_state(|| false);
        let mut done_for_future = done;
        hooks.use_future(async move {
            smol::Timer::after(Duration::from_millis(150)).await;
            done_for_future.set(true);
        });
        if done.get() {
            system.exit();
        }
        element! {
            View(flex_direction: FlexDirection::Column) {
                EchoChild(on_key: move |count: u8| {
                    let mut mirrored = mirrored_for_child;
                    mirrored.set(count);
                })
                Text(content: format!("mirrored={}", mirrored.get()))
            }
        }
    }

    #[apply(test!)]
    async fn test_event_writes_are_drained_before_rendering_under_push_wake() {
        use std::sync::{Arc, Mutex};
        super::set_push_wake_for_tests(Some(true));
        let rounds = Arc::new(Mutex::new(Vec::new()));
        let rounds_cb = Arc::clone(&rounds);
        // One key per frame: spaced out so each lands in its own poll pass.
        let keys = futures::stream::unfold(0u8, |sent| async move {
            if sent >= 3 {
                return None;
            }
            smol::Timer::after(Duration::from_millis(20)).await;
            Some((
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Char('a'))),
                sent + 1,
            ))
        });
        let frames = element!(EchoParentApp)
            .mock_terminal_render_loop_with_profile(
                MockTerminalConfig::with_events(keys),
                move |event| rounds_cb.lock().unwrap().push(event.phases.settle_rounds),
            )
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .await;
        super::set_push_wake_for_tests(None);
        assert!(frames.last().unwrap().contains("mirrored=3"), "{frames:?}");
        let rounds = rounds.lock().unwrap().clone();
        assert!(
            rounds.iter().all(|&r| r == 0),
            "event-phase writes reached the settle pass: settle rounds per frame = {rounds:?}"
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

// Dirty-path W2: the harvest scan descends dirty paths only.
#[cfg(test)]
mod scan_tests {
    use crate::component::SCAN_DESCENTS;
    use crate::prelude::*;
    use futures::stream::StreamExt;
    use std::time::Duration;

    const QUIET_ROWS: usize = 200;

    // A large subtree that never wakes. Whether the scan walks it cannot be
    // seen from inside (a clean component's own poll is skipped either way),
    // so the test counts the scan's descents instead.
    #[component]
    fn QuietSubtree() -> impl Into<AnyElement<'static>> {
        element! {
            View(flex_direction: FlexDirection::Column) {
                #((0..QUIET_ROWS).map(|i| element!(Text(key: i, content: format!("row {i}")))))
            }
        }
    }

    #[component]
    fn Ticker(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
        let mut system = hooks.use_context_mut::<SystemContext>();
        let ticks = hooks.use_state(|| 0u8);
        let mut ticks_for_interval = ticks;
        hooks.use_interval(
            move || ticks_for_interval.set(ticks_for_interval.get().saturating_add(1)),
            Some(Duration::from_millis(1)),
        );
        if ticks.get() >= 5 {
            system.exit();
        }
        element!(Text(content: format!("ticks={}", ticks.get())))
    }

    // Two sibling subtrees: one quiet and large, one waking every millisecond
    // (the ticker). Each tick wakes the ticker's path only, so after the one
    // arming scan the quiet rows must never be descended into again.
    #[component]
    fn SiblingSubtreesApp() -> impl Into<AnyElement<'static>> {
        element! {
            View(flex_direction: FlexDirection::Column) {
                QuietSubtree
                View { Ticker }
            }
        }
    }

    #[test]
    fn test_scan_skips_clean_sibling_subtree() {
        SCAN_DESCENTS.with(|count| count.set(0));
        super::set_push_wake_for_tests(Some(true));
        let canvases: Vec<_> = smol::block_on(
            element!(SiblingSubtreesApp)
                .mock_terminal_render_loop(MockTerminalConfig::default())
                .collect(),
        );
        super::set_push_wake_for_tests(None);
        let rendered = canvases.last().unwrap().to_string();
        assert!(rendered.contains("ticks=5"), "{rendered:?}");
        let descents = SCAN_DESCENTS.with(|count| count.get());
        // One arming scan covers the whole tree (~QUIET_ROWS + a handful);
        // each of the five ticks should then descend a handful of components
        // on the ticker's path. Walking the quiet rows once per tick would
        // put this at ten-plus full trees.
        assert!(
            descents < 2 * QUIET_ROWS,
            "scan descended {descents} components over the run; the quiet subtree was re-walked"
        );
    }

    // Plain views — no handlers, not focusable. Their only live event
    // subscription is the mouse one, so a keystroke must not wake any of them.
    #[component]
    fn PlainRows() -> impl Into<AnyElement<'static>> {
        element! {
            View(flex_direction: FlexDirection::Column) {
                #((0..QUIET_ROWS).map(|i| element! {
                    View(key: i) { Text(content: format!("row {i}")) }
                }))
            }
        }
    }

    #[component]
    fn KeyCounterApp(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
        let mut system = hooks.use_context_mut::<SystemContext>();
        let keys = hooks.use_state(|| 0u8);
        let mut keys_for_events = keys;
        hooks.use_terminal_events(move |event| {
            if let TerminalEvent::Key(_) = event {
                keys_for_events.set(keys_for_events.get().saturating_add(1));
            }
        });
        if keys.get() >= 5 {
            system.exit();
        }
        element! {
            View(flex_direction: FlexDirection::Column) {
                PlainRows
                Text(content: format!("keys={}", keys.get()))
            }
        }
    }

    #[test]
    fn test_keystrokes_do_not_wake_plain_views() {
        SCAN_DESCENTS.with(|count| count.set(0));
        super::set_push_wake_for_tests(Some(true));
        let keys = (0..5)
            .map(|_| TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Char('a'))));
        let canvases: Vec<_> = smol::block_on(
            element!(KeyCounterApp)
                .mock_terminal_render_loop(MockTerminalConfig::with_events(futures::stream::iter(
                    keys,
                )))
                .collect(),
        );
        super::set_push_wake_for_tests(None);
        let rendered = canvases.last().unwrap().to_string();
        assert!(rendered.contains("keys=5"), "{rendered:?}");
        let descents = SCAN_DESCENTS.with(|count| count.get());
        // The arming scan covers every component once (a view and a text per
        // row plus a handful); each key should then descend the root only. A
        // keystroke fanned out to the rows adds QUIET_ROWS descents per key.
        assert!(
            descents < 3 * QUIET_ROWS,
            "scan descended {descents} components over the run; keystrokes woke the plain rows"
        );
    }

    #[component]
    fn LateTicker(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
        let mut system = hooks.use_context_mut::<SystemContext>();
        let ticks = hooks.use_state(|| 0u8);
        let mut ticks_for_interval = ticks;
        hooks.use_interval(
            move || ticks_for_interval.set(ticks_for_interval.get().saturating_add(1)),
            Some(Duration::from_millis(1)),
        );
        if ticks.get() >= 2 {
            system.exit();
        }
        element!(Text(content: format!("late ticks={}", ticks.get())))
    }

    // The ticker is mounted by a later render, under a parent whose path the
    // scan has already consumed. Instantiation must re-mark that path, or the
    // post-render scan never reaches the new component and its interval is
    // never armed.
    #[component]
    fn LateMountApp(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
        let show = hooks.use_state(|| false);
        let mut show_for_future = show;
        hooks.use_future(async move {
            show_for_future.set(true);
        });
        element! {
            View {
                #(if show.get() {
                    element!(LateTicker).into_any()
                } else {
                    element!(Text(content: "waiting")).into_any()
                })
            }
        }
    }

    #[test]
    fn test_late_mounted_component_is_reachable_by_the_scan() {
        use futures::FutureExt;
        super::set_push_wake_for_tests(Some(true));
        let mut app = element!(LateMountApp);
        let frames = smol::block_on(futures::future::select(
            app.mock_terminal_render_loop(MockTerminalConfig::default())
                .map(|canvas| canvas.to_string())
                .collect::<Vec<_>>()
                .boxed_local(),
            smol::Timer::after(Duration::from_secs(5)),
        ));
        super::set_push_wake_for_tests(None);
        let frames = match frames {
            futures::future::Either::Left((frames, _)) => frames,
            futures::future::Either::Right(_) => {
                panic!("render loop parked: a late-mounted component was never scanned")
            }
        };
        assert!(
            frames.last().unwrap().contains("late ticks=2"),
            "{frames:?}"
        );
    }
}

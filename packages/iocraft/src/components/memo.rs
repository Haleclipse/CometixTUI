use crate::{AnyElement, Component, ComponentUpdater, Hooks, Props};

/// Comparator used by [`Memo`] to decide whether a memo key is unchanged.
pub type MemoComparator = fn(previous_key: &str, next_key: &str) -> bool;

/// String equality comparator for [`MemoProps::compare`].
pub fn memo_key_eq(previous_key: &str, next_key: &str) -> bool {
    previous_key == next_key
}

/// The props which can be passed to the [`Memo`] component.
#[non_exhaustive]
#[derive(Props, Default)]
pub struct MemoProps<'a> {
    /// Caller-owned render key for the memoized subtree.
    pub memo_key: String,
    /// Explicit comparator for [`Self::memo_key`]. When omitted, the wrapper
    /// behaves like a transparent fragment and does not memoize.
    pub compare: Option<MemoComparator>,
    /// The memoized subtree.
    pub children: Vec<AnyElement<'a>>,
}

/// Opt-in memo wrapper.
///
/// `Memo` reduces application-level memo boilerplate without changing ordinary
/// component semantics. It only skips child updates when the caller supplies an
/// explicit comparator and that comparator reports the memo key unchanged. Child
/// state/focus/input changes are still honored: if a retained child signaled an
/// internal change while the wrapper was being polled, the children update even
/// when the memo key is equal.
#[derive(Default)]
pub struct Memo {
    previous_key: Option<String>,
}

/// Alias for callers that prefer the CC Ink-aligned component name.
pub type MemoComponent = Memo;

impl Component for Memo {
    type Props<'a> = MemoProps<'a>;

    fn new(_props: &Self::Props<'_>) -> Self {
        Self::default()
    }

    fn update(
        &mut self,
        props: &mut Self::Props<'_>,
        _hooks: Hooks,
        updater: &mut ComponentUpdater,
    ) {
        updater.set_transparent_layout(true);
        let child_changed = updater.children_have_pending_change();
        let memo_equal = self
            .previous_key
            .as_deref()
            .zip(props.compare)
            .is_some_and(|(previous, compare)| compare(previous, &props.memo_key));

        if !memo_equal || child_changed {
            updater.update_children(props.children.iter_mut(), None);
        } else {
            updater.retain_children();
        }

        self.previous_key = Some(props.memo_key.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;
    use futures::StreamExt;
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

    static STABLE_CHILD_RENDERS: AtomicUsize = AtomicUsize::new(0);
    static CHANGING_CHILD_RENDERS: AtomicUsize = AtomicUsize::new(0);
    static NO_COMPARATOR_CHILD_RENDERS: AtomicUsize = AtomicUsize::new(0);

    #[derive(Default, Props)]
    struct CountingChildProps {
        label: String,
    }

    fn counting_child_text(
        props: &CountingChildProps,
        counter: &AtomicUsize,
    ) -> AnyElement<'static> {
        let renders = counter.fetch_add(1, Ordering::SeqCst) + 1;
        element!(Text(content: format!("{} renders={}", props.label, renders))).into()
    }

    #[component]
    fn StableCountingChild(props: &CountingChildProps) -> impl Into<AnyElement<'static>> {
        counting_child_text(props, &STABLE_CHILD_RENDERS)
    }

    #[component]
    fn ChangingCountingChild(props: &CountingChildProps) -> impl Into<AnyElement<'static>> {
        counting_child_text(props, &CHANGING_CHILD_RENDERS)
    }

    #[component]
    fn NoComparatorCountingChild(props: &CountingChildProps) -> impl Into<AnyElement<'static>> {
        counting_child_text(props, &NO_COMPARATOR_CHILD_RENDERS)
    }

    #[component]
    fn TransparentMemoLayoutApp() -> impl Into<AnyElement<'static>> {
        element! {
            View(flex_direction: FlexDirection::Column) {
                Text(content: "before")
                Memo(memo_key: "stable".to_string(), compare: memo_key_eq as MemoComparator) {
                    Text(content: "line 1")
                    Text(content: "line 2")
                }
                Text(content: "after")
            }
        }
    }

    #[test]
    fn test_memo_is_layout_transparent_like_react_memo() {
        assert_eq!(
            element!(TransparentMemoLayoutApp).to_string(),
            "before\nline 1\nline 2\nafter\n"
        );
    }

    #[component]
    fn StableMemoApp(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
        let mut system = hooks.use_context_mut::<SystemContext>();
        let mut tick = hooks.use_state(|| 0u8);
        if tick.get() < 3 {
            tick += 1;
        } else {
            system.exit();
        }

        element! {
            Memo(memo_key: "stable".to_string(), compare: memo_key_eq as MemoComparator) {
                StableCountingChild(label: format!("tick {}", tick.get()))
            }
        }
    }

    #[test]
    fn test_memo_equal_comparator_skips_child_update() {
        STABLE_CHILD_RENDERS.store(0, Ordering::SeqCst);
        let canvases: Vec<_> = smol::block_on(
            element!(StableMemoApp)
                .mock_terminal_render_loop(MockTerminalConfig::default())
                .collect(),
        );
        let rendered = canvases.last().unwrap().to_string();
        assert!(
            rendered.starts_with("tick 1 renders=1"),
            "stable memo key should keep the initial child render: {rendered:?}"
        );
    }

    #[component]
    fn ChangingMemoApp(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
        let mut system = hooks.use_context_mut::<SystemContext>();
        let mut tick = hooks.use_state(|| 0u8);
        if tick.get() < 3 {
            tick += 1;
        } else {
            system.exit();
        }

        element! {
            Memo(memo_key: format!("tick-{}", tick.get()), compare: memo_key_eq as MemoComparator) {
                ChangingCountingChild(label: format!("tick {}", tick.get()))
            }
        }
    }

    #[test]
    fn test_memo_changed_comparator_rerenders_child() {
        CHANGING_CHILD_RENDERS.store(0, Ordering::SeqCst);
        let canvases: Vec<_> = smol::block_on(
            element!(ChangingMemoApp)
                .mock_terminal_render_loop(MockTerminalConfig::default())
                .collect(),
        );
        let rendered = canvases.last().unwrap().to_string();
        assert!(
            rendered.starts_with("tick 3 renders=3"),
            "changing memo key should update the child every frame: {rendered:?}"
        );
    }

    static BLIT_PROBE_DRAWS: AtomicUsize = AtomicUsize::new(0);

    #[derive(Default)]
    struct DrawProbeHook;
    impl crate::Hook for DrawProbeHook {
        fn pre_component_draw(&mut self, _drawer: &mut crate::ComponentDrawer) {
            BLIT_PROBE_DRAWS.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[component]
    fn DrawProbedChild(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
        hooks.use_hook(DrawProbeHook::default);
        element!(Text(content: "blit-stable"))
    }

    #[component]
    fn BlitProbeApp(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
        let mut system = hooks.use_context_mut::<SystemContext>();
        let mut tick = hooks.use_state(|| 0u8);
        if tick.get() < 3 {
            tick += 1;
        } else {
            system.exit();
        }

        element! {
            View(flex_direction: FlexDirection::Column) {
                Text(content: format!("tick {}", tick.get()))
                Memo(memo_key: "stable".to_string(), compare: memo_key_eq as MemoComparator) {
                    DrawProbedChild
                }
            }
        }
    }

    /// The retained-blit fast path must skip the entire subtree draw (hooks
    /// included) once the memo has retained its children and the layout is
    /// unchanged, while keeping the subtree's cells on the canvas.
    #[test]
    fn test_retained_blit_skips_child_draw_when_enabled() {
        crate::render::set_retained_blit_for_tests(Some(true));
        BLIT_PROBE_DRAWS.store(0, Ordering::SeqCst);
        let canvases: Vec<_> = smol::block_on(
            element!(BlitProbeApp)
                .mock_terminal_render_loop(MockTerminalConfig::default())
                .collect(),
        );
        crate::render::set_retained_blit_for_tests(None);
        let frames = canvases.len();
        let draws = BLIT_PROBE_DRAWS.load(Ordering::SeqCst);
        let rendered = canvases.last().unwrap().to_string();
        assert!(
            rendered.contains("blit-stable"),
            "blitted subtree must keep its cells: {rendered:?}"
        );
        assert!(
            rendered.contains("tick 3"),
            "dynamic sibling must keep updating: {rendered:?}"
        );
        assert!(
            draws < frames,
            "retained blit should skip child draws after the first frame: draws={draws} frames={frames}"
        );
    }

    #[derive(Default, Props)]
    struct SettleRelayProps {
        value: u8,
    }

    /// Mirrors its prop into state during render — a render-phase write, so
    /// the frame that changes the prop takes a settle round.
    #[component]
    fn SettleRelay(props: &SettleRelayProps, mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
        let mut seen = hooks.use_state(|| 0u8);
        if seen.get() != props.value {
            seen.set(props.value);
        }
        element!(Text(content: format!("relay {}", seen.get())))
    }

    #[component]
    fn SettleBlitApp(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
        let mut value = hooks.use_state(|| 0u8);
        hooks.use_terminal_events(move |event| {
            if matches!(event, TerminalEvent::Key(key) if key.kind == KeyEventKind::Press) {
                value.set(value.get() + 1);
            }
        });
        element! {
            View(flex_direction: FlexDirection::Column) {
                Memo(memo_key: format!("value {}", value.get()), compare: memo_key_eq as MemoComparator) {
                    Text(content: format!("memo {}", value.get()))
                }
                SettleRelay(value: value.get())
            }
        }
    }

    /// A memo whose key changed re-enters its children in the frame's first
    /// update pass and retains them in the settle round that follows. The
    /// previous canvas predates the first pass, so the retained-blit must not
    /// restore the subtree from it.
    #[test]
    fn test_retained_blit_skips_a_memo_that_re_rendered_earlier_in_the_frame() {
        use std::sync::{Arc, Mutex};
        crate::render::set_retained_blit_for_tests(Some(true));
        crate::render::wake::set_push_wake_for_tests(Some(true));
        let settle_rounds = Arc::new(Mutex::new(Vec::new()));
        let rounds = Arc::clone(&settle_rounds);
        let frames = smol::block_on(async move {
            let (keys, events) = futures::channel::mpsc::unbounded();
            let mut app = element!(SettleBlitApp);
            let mut render_loop = Box::pin(app.mock_terminal_render_loop_with_profile(
                MockTerminalConfig::with_events(events),
                move |profile| rounds.lock().unwrap().push(profile.phases.settle_rounds),
            ));
            let mut frames = Vec::new();
            while let Some(canvas) = render_loop.next().await {
                let text = canvas.to_string();
                frames.push(text.clone());
                if text.contains("relay 1") {
                    break;
                }
                if frames.len() == 1 {
                    keys.unbounded_send(TerminalEvent::Key(KeyEvent::new(
                        KeyEventKind::Press,
                        KeyCode::Char('a'),
                    )))
                    .unwrap();
                }
            }
            frames
        });
        crate::render::wake::set_push_wake_for_tests(None);
        crate::render::set_retained_blit_for_tests(None);
        let last = frames.last().unwrap();
        assert!(last.contains("relay 1"), "{frames:?}");
        assert!(
            settle_rounds.lock().unwrap().iter().any(|&rounds| rounds > 0),
            "the key frame must take a settle round for this to test anything"
        );
        assert!(
            last.contains("memo 1"),
            "the re-rendered memo was blitted back from the previous frame: {frames:?}"
        );
    }

    #[component]
    fn NoComparatorMemoApp(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
        let mut system = hooks.use_context_mut::<SystemContext>();
        let mut tick = hooks.use_state(|| 0u8);
        if tick.get() < 3 {
            tick += 1;
        } else {
            system.exit();
        }

        element! {
            Memo(memo_key: "stable".to_string()) {
                NoComparatorCountingChild(label: format!("tick {}", tick.get()))
            }
        }
    }

    #[test]
    fn test_memo_requires_explicit_comparator() {
        NO_COMPARATOR_CHILD_RENDERS.store(0, Ordering::SeqCst);
        let canvases: Vec<_> = smol::block_on(
            element!(NoComparatorMemoApp)
                .mock_terminal_render_loop(MockTerminalConfig::default())
                .collect(),
        );
        let rendered = canvases.last().unwrap().to_string();
        assert!(
            rendered.starts_with("tick 3 renders=4"),
            "missing comparator should preserve normal child updates: {rendered:?}"
        );
    }

    static PULSE_RENDERED: AtomicUsize = AtomicUsize::new(0);

    #[component]
    fn PulsingChild(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
        let pulse = hooks.use_state(|| 0u8);
        let mut pulse_for_interval = pulse;
        hooks.use_interval(
            move || {
                if pulse_for_interval.get() < 3 {
                    pulse_for_interval += 1;
                }
            },
            Some(Duration::from_millis(1)),
        );
        // Published from the render, not the timer callback: the parent must
        // only exit once this value has reached the canvas, or a stable memo
        // key can leave the last frame one pulse behind.
        PULSE_RENDERED.store(pulse.get() as usize, Ordering::SeqCst);
        element!(Text(content: format!("pulse={}", pulse.get())))
    }

    #[component]
    fn StatefulChildMemoApp(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
        let mut system = hooks.use_context_mut::<SystemContext>();
        // The parent keeps re-rendering on its own clock while the memo key
        // stays stable. The clock ticks from outside the render (a render-phase
        // bump is folded into one frame by push-mode settle), and the app only
        // exits once the child has rendered its final pulse, so the assertion
        // never races the two timers.
        let tick = hooks.use_state(|| 0u8);
        let mut tick_for_interval = tick;
        hooks.use_interval(
            move || tick_for_interval.set(tick_for_interval.get().saturating_add(1)),
            Some(Duration::from_millis(1)),
        );
        if tick.get() >= 8 && PULSE_RENDERED.load(Ordering::SeqCst) >= 3 {
            system.exit();
        }

        element! {
            Memo(memo_key: "stable".to_string(), compare: memo_key_eq as MemoComparator) {
                PulsingChild
            }
        }
    }

    #[test]
    fn test_memo_does_not_skip_stateful_child_changes() {
        PULSE_RENDERED.store(0, Ordering::SeqCst);
        let canvases: Vec<_> = smol::block_on(
            element!(StatefulChildMemoApp)
                .mock_terminal_render_loop(MockTerminalConfig::default())
                .collect(),
        );
        let rendered = canvases.last().unwrap().to_string();
        assert!(
            rendered.starts_with("pulse=3"),
            "child hook polling should still update through a stable memo key: {rendered:?}"
        );
    }
}

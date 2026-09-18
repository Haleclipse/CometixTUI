use crate::{strip_ansi::strip_ansi, ComponentUpdater, Hook, Hooks};

mod private {
    pub trait Sealed {}
    impl Sealed for crate::Hooks<'_, '_> {}
}

/// Declaratively set the terminal tab/window title.
///
/// This mirrors the CC Ink fork's `useTerminalTitle(...)`: ANSI escape
/// sequences are stripped before the title is handed to the terminal, and
/// `None` is a no-op that leaves any existing title untouched.
///
/// Like the original `useEffect(..., [title])`, the title is written only on
/// the render pass where it changes. Re-rendering with an unchanged title
/// emits nothing, which matters on terminals such as Termux that treat any
/// incoming bytes as a reason to scroll back to the bottom.
///
/// The write happens synchronously in the same update pass, so a component
/// that sets a title and calls [`SystemContext::exit`](crate::SystemContext::exit)
/// in its first render still delivers the title before the loop exits, just as
/// React flushes passive effects before Ink unmounts.
///
/// Delivery is decided by the terminal backend: OSC 0 on Unix-likes, and
/// crossterm's `SetTitle` on Windows so legacy conhost without VT support
/// still receives `SetConsoleTitleW` (the Rust counterpart to CC Ink's
/// `process.title` branch).
pub trait UseTerminalTitle: private::Sealed {
    /// Sets the terminal title. The write happens once per distinct title.
    fn use_terminal_title<S>(&mut self, title: S)
    where
        S: Into<String>;

    /// Sets the terminal title when `title` is `Some`, or leaves it untouched
    /// when `None`.
    fn use_terminal_title_opt<S>(&mut self, title: Option<S>)
    where
        S: Into<String>;
}

impl UseTerminalTitle for Hooks<'_, '_> {
    fn use_terminal_title<S>(&mut self, title: S)
    where
        S: Into<String>,
    {
        self.use_terminal_title_opt(Some(title));
    }

    fn use_terminal_title_opt<S>(&mut self, title: Option<S>)
    where
        S: Into<String>,
    {
        let clean = title.map(|title| strip_ansi(&title.into()).into_owned());
        let hook = self.use_hook(UseTerminalTitleImpl::default);
        // Same comparison React makes on `[title]`: `None` participates, so
        // `Some(a) -> None -> Some(a)` re-asserts the title, while an unchanged
        // value schedules nothing.
        if hook.last_title != clean {
            hook.pending = clean.clone();
            hook.last_title = clean;
        }
    }
}

#[derive(Default)]
struct UseTerminalTitleImpl {
    /// The dependency as of the last render (`None` on the very first render,
    /// matching an initial `null` title in the original).
    last_title: Option<String>,
    /// Title to deliver after this update pass, if the dependency changed to
    /// `Some`.
    pending: Option<String>,
}

impl Hook for UseTerminalTitleImpl {
    fn post_component_update(&mut self, updater: &mut ComponentUpdater) {
        let Some(title) = self.pending.take() else {
            return;
        };
        // Without a terminal (e.g. rendering to a string) the original's
        // `writeRaw` is a no-op as well; the dependency is still recorded so
        // nothing is retried later.
        if let Some(terminal) = updater.terminal_mut() {
            let _ = terminal.set_title(&title);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;
    use futures::{channel::mpsc, StreamExt};

    #[derive(Default, Props)]
    struct TitleScriptProps {
        script: Vec<Option<&'static str>>,
    }

    /// Drives one render per key press. The title for each render comes from
    /// `script`, so a test can stage "unchanged", "changed", and `None` phases.
    /// The render that runs off the end of the script exits.
    #[component]
    fn TitleScript(mut hooks: Hooks, props: &TitleScriptProps) -> impl Into<AnyElement<'static>> {
        let mut system = hooks.use_context_mut::<SystemContext>();
        let mut presses = hooks.use_state(|| 0usize);
        hooks.use_terminal_events(move |event| {
            if let TerminalEvent::Key(KeyEvent {
                kind: KeyEventKind::Press,
                ..
            }) = event
            {
                presses += 1;
            }
        });
        let step = presses.get();
        hooks.use_terminal_title_opt(props.script.get(step).copied().flatten());
        if step >= props.script.len() {
            system.exit();
        }
        element!(Text(content: format!("step{step}")))
    }

    fn run_script(script: Vec<Option<&'static str>>) -> Vec<String> {
        smol::block_on(async {
            let (title_tx, title_rx) = mpsc::unbounded();
            let (event_tx, event_rx) = mpsc::unbounded();
            let steps = script.len();
            let mut app = element!(TitleScript(script: script));
            let mut frames = app.mock_terminal_render_loop(
                MockTerminalConfig::with_events(event_rx).with_title_sink(title_tx),
            );
            for step in 0..=steps {
                if step > 0 {
                    event_tx
                        .unbounded_send(TerminalEvent::Key(KeyEvent::new(
                            KeyEventKind::Press,
                            KeyCode::Char('n'),
                        )))
                        .unwrap();
                }
                // Wait for the frame that acknowledges this step so the next
                // key is only sent once this render pass has been committed.
                let marker = format!("step{step}");
                while let Some(canvas) = frames.next().await {
                    if canvas.to_string().contains(&marker) {
                        break;
                    }
                }
            }
            // Drain until the render loop exits; dropping the mock terminal
            // closes the title sink so `collect` terminates.
            while frames.next().await.is_some() {}
            drop(frames);
            title_rx.collect::<Vec<_>>().await
        })
    }

    #[test]
    fn test_use_terminal_title_writes_once_for_unchanged_title() {
        // Three renders with the same title must produce exactly one write:
        // re-writing an unchanged title makes Termux scroll back to the bottom
        // on every idle render pass.
        assert_eq!(
            run_script(vec![Some("Same"), Some("Same"), Some("Same")]),
            vec!["Same".to_string()]
        );
    }

    #[test]
    fn test_use_terminal_title_writes_on_change_and_strips_ansi() {
        assert_eq!(
            run_script(vec![
                Some("\x1b[31mAlpha\x1b[0m"),
                Some("Alpha"),
                Some("\x1b[1mBeta\x1b[0m"),
            ]),
            vec!["Alpha".to_string(), "Beta".to_string()]
        );
    }

    #[test]
    fn test_use_terminal_title_none_is_noop_and_reasserts_after() {
        assert_eq!(
            run_script(vec![Some("Keep"), None, None, Some("Keep")]),
            vec!["Keep".to_string(), "Keep".to_string()]
        );
    }

    #[test]
    fn test_use_terminal_title_only_none_never_writes() {
        assert!(run_script(vec![None, None]).is_empty());
    }

    /// Sets a title and exits in the very same (first) render, like
    /// `examples/terminal_title.rs`.
    #[component]
    fn TitleThenExit(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
        let mut system = hooks.use_context_mut::<SystemContext>();
        hooks.use_terminal_title("\x1b[32mFinal\x1b[0m");
        system.exit();
        element!(Text(content: "bye"))
    }

    #[test]
    fn test_use_terminal_title_is_delivered_when_exiting_in_same_render() {
        // The title must not be lost just because the loop exits right after
        // this pass; the original flushes passive effects before unmounting.
        let titles = smol::block_on(async {
            let (title_tx, title_rx) = mpsc::unbounded();
            let mut app = element!(TitleThenExit);
            let mut frames = app
                .mock_terminal_render_loop(MockTerminalConfig::default().with_title_sink(title_tx));
            while frames.next().await.is_some() {}
            drop(frames);
            title_rx.collect::<Vec<_>>().await
        });
        assert_eq!(titles, vec!["Final".to_string()]);
    }

    #[component]
    fn TitleProbe(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
        hooks.use_terminal_title("\x1b[31mClean\x1b[0m Title");
        hooks.use_terminal_title_opt::<String>(None);
        element!(Text(content: "rendered"))
    }

    #[test]
    fn test_use_terminal_title_renders_without_terminal() {
        // Without a terminal the write is a no-op; rendering must not panic
        // and hook order must be stable across `Some`/`None`.
        assert_eq!(element!(TitleProbe).to_string(), "rendered\n");
    }
}

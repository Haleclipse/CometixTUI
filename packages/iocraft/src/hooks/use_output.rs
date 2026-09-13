use super::{SelectionClipboardPath, SelectionContext, UseContext};
use crate::{
    Canvas, Clipboard, ClipboardMultiplexer, ComponentUpdater, Hook, Hooks, SelectionController,
    SelectionState,
};
use core::{
    pin::Pin,
    task::{Context, Poll, Waker},
};
use crossterm::{cursor, QueueableCommand};
use futures::{channel::oneshot, future::BoxFuture};
use std::io;
use std::{
    borrow::Cow,
    sync::{Arc, Mutex},
};
use unicode_width::UnicodeWidthChar;

mod private {
    pub trait Sealed {}
    impl Sealed for crate::Hooks<'_, '_> {}
}

/// `UseOutput` is a hook that allows you to write to stdout and stderr from a component. The
/// output will be appended to stdout or stderr, above the rendered component output.
///
/// Both `print` and `println` methods are available for writing output with or without newlines.
///
/// # Example
///
/// ```
/// # use iocraft::prelude::*;
/// # use std::time::Duration;
/// #[component]
/// fn Example(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
///     let (stdout, stderr) = hooks.use_output();
///
///     hooks.use_future(async move {
///         stdout.println("Hello from iocraft to stdout!");
///         stderr.println("  And hello to stderr too!");
///
///         stdout.print("Working...");
///         for _ in 0..5 {
///             smol::Timer::after(Duration::from_secs(1)).await;
///             stdout.print(".");
///         }
///         stdout.println("\nDone!");
///     });
///
///     element! {
///         View(border_style: BorderStyle::Round, border_color: Color::Green) {
///             Text(content: "Hello, use_output!")
///         }
///     }
/// }
/// ```
pub trait UseOutput: private::Sealed {
    /// Gets handles which can be used to write to stdout and stderr.
    fn use_output(&mut self) -> (StdoutHandle, StderrHandle);
}

impl UseOutput for Hooks<'_, '_> {
    fn use_output(&mut self) -> (StdoutHandle, StderrHandle) {
        let clipboard = self
            .try_use_context::<Clipboard>()
            .map(|value| value.clone());
        let output = self.use_hook(UseOutputImpl::default);
        output.clipboard = clipboard;
        (output.use_stdout(), output.use_stderr())
    }
}

enum Message {
    Stdout(String),
    StdoutNoNewline(String),
    StdoutClipboard(String, ClipboardMultiplexer),
    StdoutControl(String),
    StdoutControlAcknowledged(String, oneshot::Sender<io::Result<()>>),
    Stderr(String),
    StderrNoNewline(String),
}

impl Message {
    fn affects_visible_output(&self) -> bool {
        !matches!(
            self,
            Message::StdoutClipboard(..)
                | Message::StdoutControl(_)
                | Message::StdoutControlAcknowledged(..)
        )
    }
}

#[derive(Default)]
struct UseOutputState {
    closed: bool,
    queue: Vec<Message>,
    waker: Option<Waker>,
    appended_newline: Option<u16>,
}

fn normalize_terminal_newlines(text: &str) -> Cow<'_, str> {
    let mut prev = '\0';
    let mut normalized = None::<String>;
    for (idx, ch) in text.char_indices() {
        if ch == '\n' && prev != '\r' {
            let out = normalized.get_or_insert_with(|| {
                let mut s = String::with_capacity(text.len() + 1);
                s.push_str(&text[..idx]);
                s
            });
            out.push('\r');
            out.push('\n');
        } else if let Some(out) = normalized.as_mut() {
            out.push(ch);
        }
        prev = ch;
    }
    normalized.map(Cow::Owned).unwrap_or(Cow::Borrowed(text))
}

fn skip_ansi_string_control(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    let mut saw_escape = false;
    for ch in chars.by_ref() {
        if ch == '\u{7}' {
            break;
        }
        if saw_escape && ch == '\\' {
            break;
        }
        saw_escape = ch == '\u{1b}';
    }
}

fn skip_ansi_escape(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    let Some(kind) = chars.next() else {
        return;
    };
    match kind {
        '[' => {
            for ch in chars.by_ref() {
                let code = ch as u32;
                if (0x40..=0x7e).contains(&code) {
                    break;
                }
            }
        }
        ']' | 'P' | '_' | '^' | 'X' => skip_ansi_string_control(chars),
        _ => {}
    }
}

fn advance_printable_width(col: u16, char_width: usize, terminal_width: Option<usize>) -> u16 {
    if char_width == 0 {
        return col;
    }

    let Some(terminal_width) = terminal_width.filter(|w| *w > 0) else {
        return (col as usize)
            .saturating_add(char_width)
            .min(u16::MAX as usize) as u16;
    };

    // `col == terminal_width` represents the VT pending-wrap state after a
    // printable ended exactly at the right margin. The next printable first
    // wraps to the following row before drawing at column 0.
    let start_col = if col as usize >= terminal_width {
        0
    } else {
        col as usize
    };
    let advanced = start_col.saturating_add(char_width);
    if advanced <= terminal_width {
        // Preserve `terminal_width` as the pending-wrap marker.
        advanced as u16
    } else {
        (advanced % terminal_width) as u16
    }
}

fn advance_terminal_column(col: u16, text: &str, terminal_width: Option<usize>) -> u16 {
    let mut col = col;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\u{1b}' => skip_ansi_escape(&mut chars),
            '\r' | '\n' => col = 0,
            '\t' => {
                let next_tab = ((col as usize / 8) + 1) * 8;
                let delta = next_tab.saturating_sub(col as usize);
                col = advance_printable_width(col, delta, terminal_width);
            }
            '\u{8}' => col = col.saturating_sub(1),
            _ if ch.is_control() => {}
            _ => {
                col = advance_printable_width(col, ch.width().unwrap_or(0), terminal_width);
            }
        }
    }
    col
}

impl UseOutputState {
    fn exec(&mut self, updater: &mut ComponentUpdater) {
        if self.queue.is_empty() {
            return;
        }

        // Check if we have a terminal - if not, messages stay queued
        if updater.terminal_mut().is_none() {
            return;
        }

        let has_visible_output = self.queue.iter().any(Message::affects_visible_output);
        if has_visible_output {
            updater.clear_terminal_output();
        }
        let terminal = updater.terminal_mut().unwrap();
        let terminal_width = terminal.size().map(|(width, _)| width as usize);

        if has_visible_output {
            if let Some(col) = self.appended_newline {
                let _ = terminal
                    .render_output()
                    .queue(cursor::MoveUp(1))
                    .and_then(|w| w.queue(cursor::MoveRight(col)));
            }
            // Flush render output to ensure escape sequences are sent before any
            // cross-stream writes (e.g., stdout messages when rendering to stderr).
            let _ = terminal.render_output().flush();
        }

        // Track the virtual cursor column ourselves instead of querying the
        // terminal. Crossterm's cursor::position() races with EventStream's
        // raw-mode stdin reader and can fail or steal bytes; when that happens
        // no extra newline is emitted and the next canvas frame is painted on
        // the same row as the progress output (visible tearing in kitty).
        let mut output_col = self.appended_newline.unwrap_or(0);

        for msg in self.queue.drain(..) {
            match msg {
                Message::Stdout(msg) => {
                    let mut formatted = normalize_terminal_newlines(&msg).into_owned();
                    formatted.push_str("\r\n");
                    let _ = terminal.stdout().write_all(formatted.as_bytes());
                    output_col = 0;
                }
                Message::StdoutNoNewline(msg) => {
                    let formatted = normalize_terminal_newlines(&msg);
                    let _ = terminal.stdout().write_all(formatted.as_bytes());
                    output_col = advance_terminal_column(output_col, &msg, terminal_width);
                }
                Message::StdoutClipboard(msg, multiplexer) => {
                    let _ = terminal.set_clipboard_with_multiplexer(&msg, multiplexer);
                }
                Message::StdoutControl(sequence) => {
                    let _ = terminal.write_control_sequence(&sequence);
                }
                Message::StdoutControlAcknowledged(sequence, sender) => {
                    let result = terminal.write_control_sequence(&sequence);
                    let _ = sender.send(result);
                }
                Message::Stderr(msg) => {
                    let mut formatted = normalize_terminal_newlines(&msg).into_owned();
                    formatted.push_str("\r\n");
                    let _ = terminal.stderr().write_all(formatted.as_bytes());
                    output_col = 0;
                }
                Message::StderrNoNewline(msg) => {
                    let formatted = normalize_terminal_newlines(&msg);
                    let _ = terminal.stderr().write_all(formatted.as_bytes());
                    output_col = advance_terminal_column(output_col, &msg, terminal_width);
                }
            }
        }

        if has_visible_output {
            // Flush stdout and stderr so the terminal processes all written
            // bytes before we append the trailing newline.
            let _ = terminal.stdout().flush();
            let _ = terminal.stderr().flush();
            if output_col > 0 {
                self.appended_newline = match terminal_width {
                    Some(width) if output_col as usize >= width => None,
                    _ => Some(output_col),
                };
                let _ = terminal.render_output().write_all(b"\r\n");
            } else {
                self.appended_newline = None;
            }
        }
    }
}

/// A handle to write to stdout, obtained from [`UseOutput::use_output`].
#[derive(Clone)]
pub struct StdoutHandle {
    state: Arc<Mutex<UseOutputState>>,
    clipboard: Option<Clipboard>,
}

impl StdoutHandle {
    pub(crate) fn without_clipboard(mut self) -> Self {
        self.clipboard = None;
        self
    }

    /// Starts native/tmux clipboard work and returns the sequence to write.
    /// This intentionally does not wait for native clipboard completion.
    pub fn prepare_clipboard(&self, text: &str) -> BoxFuture<'static, String> {
        if let Some(clipboard) = &self.clipboard {
            clipboard.set_clipboard(text)
        } else {
            let sequence = crate::ansi::osc52_clipboard_sequence(text);
            Box::pin(async move { sequence })
        }
    }

    /// Notification confidence, resolved by the shared clipboard policy.
    pub fn get_clipboard_path(&self) -> SelectionClipboardPath {
        self.clipboard
            .as_ref()
            .map(Clipboard::get_clipboard_path)
            .unwrap_or_default()
    }

    /// Queues raw bytes immediately. Resolves after the terminal writer accepts
    /// them. A bound Clipboard routes to its retained root queue, surviving
    /// child unmount. Unmounting that output owner reports BrokenPipe.
    pub fn write_control_sequence_and_wait<S: ToString>(
        &self,
        sequence: S,
    ) -> BoxFuture<'static, io::Result<()>> {
        let sequence = sequence.to_string();
        if let Some(root) = self.clipboard.as_ref().and_then(Clipboard::output) {
            return root.enqueue_control_sequence_and_wait(sequence);
        }
        self.enqueue_control_sequence_and_wait(sequence)
    }

    fn enqueue_control_sequence_and_wait(
        &self,
        sequence: String,
    ) -> BoxFuture<'static, io::Result<()>> {
        let (sender, receiver) = oneshot::channel();
        let mut state = self.state.lock().unwrap();
        if !state.closed {
            state
                .queue
                .push(Message::StdoutControlAcknowledged(sequence, sender));
            if let Some(waker) = state.waker.take() {
                waker.wake();
            }
        }
        Box::pin(async move {
            receiver.await.unwrap_or_else(|_| {
                Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "terminal output owner was unmounted",
                ))
            })
        })
    }

    /// Queues a message to be written asynchronously to stdout, above the rendered component
    /// output.
    pub fn println<S: ToString>(&self, msg: S) {
        let mut state = self.state.lock().unwrap();
        state.queue.push(Message::Stdout(msg.to_string()));
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    }

    /// Queues a message to be written asynchronously to stdout without a newline, above the
    /// rendered component output.
    pub fn print<S: ToString>(&self, msg: S) {
        let msg = msg.to_string();
        if msg.is_empty() {
            return;
        }
        let mut state = self.state.lock().unwrap();
        state.queue.push(Message::StdoutNoNewline(msg));
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    }

    /// Queues a raw terminal control sequence to stdout.
    ///
    /// Unlike [`StdoutHandle::print`], this is non-visual output: it does not
    /// clear or reposition the retained render canvas. It is intended for OSC
    /// notifications, terminal progress, and other side-band terminal controls.
    pub fn write_control_sequence<S: ToString>(&self, sequence: S) {
        let sequence = sequence.to_string();
        if sequence.is_empty() {
            return;
        }
        let mut state = self.state.lock().unwrap();
        state.queue.push(Message::StdoutControl(sequence));
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    }

    /// Queues an OSC 52 clipboard write to stdout.
    ///
    /// Unlike [`StdoutHandle::print`], this is a non-visual terminal control
    /// sequence: it does not clear or reposition the retained render canvas.
    /// This makes it suitable for fullscreen text-selection copy behavior.
    pub fn set_clipboard<S: ToString>(&self, text: S) {
        if let Some(clipboard) = &self.clipboard {
            let prepared = clipboard.set_clipboard(&text.to_string());
            let stdout = self.clone();
            clipboard.spawn(Box::pin(async move {
                let sequence = prepared.await;
                let _ = stdout.write_control_sequence_and_wait(sequence).await;
            }));
            return;
        }
        self.set_clipboard_with_multiplexer(text, ClipboardMultiplexer::None);
    }

    /// Queues an OSC 52 clipboard write to stdout using an explicit
    /// multiplexer passthrough wrapper.
    pub fn set_clipboard_with_multiplexer<S: ToString>(
        &self,
        text: S,
        multiplexer: ClipboardMultiplexer,
    ) {
        let mut state = self.state.lock().unwrap();
        state
            .queue
            .push(Message::StdoutClipboard(text.to_string(), multiplexer));
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    }

    /// Copies a fullscreen [`SelectionState`] from a [`Canvas`] to the terminal
    /// clipboard without clearing the highlight. Returns the selected text.
    pub fn copy_selection_no_clear(&self, selection: &SelectionState, canvas: &Canvas) -> String {
        if !selection.has_selection() {
            return String::new();
        }
        let text = selection.selected_text(canvas);
        if !text.is_empty() {
            self.set_clipboard(&text);
        }
        text
    }

    /// Copies a fullscreen [`SelectionState`] from a [`Canvas`] to the terminal
    /// clipboard and clears the selection. Returns the selected text.
    pub fn copy_selection(&self, selection: &mut SelectionState, canvas: &Canvas) -> String {
        if !selection.has_selection() {
            return String::new();
        }
        let text = self.copy_selection_no_clear(selection, canvas);
        selection.clear();
        text
    }

    /// Copies from an app-level [`SelectionContext`] without clearing the
    /// highlight. Returns the selected text.
    ///
    /// This is the public app-level counterpart to CC Ink's
    /// `useSelection().copySelectionNoClear()`: the selection owner stays in a
    /// shared context while clipboard transport remains on `StdoutHandle`.
    pub fn copy_selection_context_no_clear(
        &self,
        selection: &SelectionContext,
        canvas: &Canvas,
    ) -> String {
        let text = selection.copy_selection_no_clear_text(canvas);
        if !text.is_empty() {
            self.set_clipboard(&text);
        }
        text
    }

    /// Copies from an app-level [`SelectionContext`] and clears the highlight.
    /// Returns the selected text.
    pub fn copy_selection_context(&self, selection: &SelectionContext, canvas: &Canvas) -> String {
        let text = selection.copy_selection_text(canvas);
        if !text.is_empty() {
            self.set_clipboard(&text);
        }
        text
    }

    /// Runs CC Ink-style copy-on-select for an app-level [`SelectionContext`].
    pub fn copy_on_select_context(
        &self,
        selection: &SelectionContext,
        canvas: &Canvas,
    ) -> Option<String> {
        let text = selection.copy_on_select_text(canvas)?;
        self.set_clipboard(&text);
        Some(text)
    }

    /// Runs app-level copy-on-select with an explicit multiplexer passthrough wrapper.
    pub fn copy_on_select_context_with_multiplexer(
        &self,
        selection: &SelectionContext,
        canvas: &Canvas,
        multiplexer: ClipboardMultiplexer,
    ) -> Option<String> {
        let text = selection.copy_on_select_text(canvas)?;
        self.set_clipboard_with_multiplexer(&text, multiplexer);
        Some(text)
    }

    /// Runs CC Ink-style copy-on-select for a [`SelectionController`].
    ///
    /// When a selection has just settled, this queues an OSC 52 clipboard write
    /// and returns the copied text. Repeated calls for the same settled
    /// selection return `None` until a new drag/selection resets the controller's
    /// copy-on-select guard.
    pub fn copy_on_select(
        &self,
        selection: &mut SelectionController,
        canvas: &Canvas,
    ) -> Option<String> {
        let text = selection.copy_on_select_text(canvas)?;
        self.set_clipboard(&text);
        Some(text)
    }

    /// Runs copy-on-select with an explicit multiplexer passthrough wrapper.
    pub fn copy_on_select_with_multiplexer(
        &self,
        selection: &mut SelectionController,
        canvas: &Canvas,
        multiplexer: ClipboardMultiplexer,
    ) -> Option<String> {
        let text = selection.copy_on_select_text(canvas)?;
        self.set_clipboard_with_multiplexer(&text, multiplexer);
        Some(text)
    }
}

/// A handle to write to stderr, obtained from [`UseOutput::use_output`].
#[derive(Clone)]
pub struct StderrHandle {
    state: Arc<Mutex<UseOutputState>>,
}

impl StderrHandle {
    /// Queues a message to be written asynchronously to stderr, above the rendered component
    /// output.
    pub fn println<S: ToString>(&self, msg: S) {
        let mut state = self.state.lock().unwrap();
        state.queue.push(Message::Stderr(msg.to_string()));
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    }

    /// Queues a message to be written asynchronously to stderr without a newline, above the
    /// rendered component output.
    pub fn print<S: ToString>(&self, msg: S) {
        let msg = msg.to_string();
        if msg.is_empty() {
            return;
        }
        let mut state = self.state.lock().unwrap();
        state.queue.push(Message::StderrNoNewline(msg));
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    }
}

#[derive(Default)]
struct UseOutputImpl {
    state: Arc<Mutex<UseOutputState>>,
    clipboard: Option<Clipboard>,
}

impl Drop for UseOutputImpl {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        state.queue.clear();
    }
}

impl Hook for UseOutputImpl {
    fn poll_change(self: Pin<&mut Self>, cx: &mut Context) -> Poll<()> {
        let mut state = self.state.lock().unwrap();
        if state.queue.is_empty() {
            state.waker = Some(cx.waker().clone());
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }

    fn post_component_update(&mut self, updater: &mut ComponentUpdater) {
        let mut state = self.state.lock().unwrap();
        state.exec(updater);
    }
}

impl UseOutputImpl {
    pub fn use_stdout(&mut self) -> StdoutHandle {
        StdoutHandle {
            state: self.state.clone(),
            clipboard: self.clipboard.clone(),
        }
    }

    pub fn use_stderr(&mut self) -> StderrHandle {
        StderrHandle {
            state: self.state.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;
    use futures::task::noop_waker;
    use macro_rules_attribute::apply;
    use smol_macros::test;

    #[test]
    fn test_normalize_terminal_newlines_preserves_carriage_return_alignment() {
        assert!(matches!(
            normalize_terminal_newlines("plain"),
            Cow::Borrowed("plain")
        ));
        assert_eq!(normalize_terminal_newlines("a\nb").as_ref(), "a\r\nb");
        assert_eq!(normalize_terminal_newlines("a\r\nb").as_ref(), "a\r\nb");
        assert_eq!(normalize_terminal_newlines("\nDone!").as_ref(), "\r\nDone!");
    }

    #[test]
    fn test_advance_terminal_column_tracks_print_continuation_without_dsr() {
        assert_eq!(advance_terminal_column(0, "Working...", None), 10);
        assert_eq!(advance_terminal_column(10, ".", None), 11);
        assert_eq!(advance_terminal_column(11, "\nDone!", None), 5);
        assert_eq!(advance_terminal_column(0, "中\t", None), 8);
    }

    #[test]
    fn test_advance_terminal_column_wraps_with_terminal_width() {
        assert_eq!(advance_terminal_column(0, "abcdef", Some(10)), 6);
        assert_eq!(advance_terminal_column(6, "abcdef", Some(10)), 2);
        assert_eq!(advance_terminal_column(0, "1234567890", Some(10)), 10);
        assert_eq!(advance_terminal_column(10, ".", Some(10)), 1);
        assert_eq!(advance_terminal_column(0, "12345\nabc", Some(10)), 3);
    }

    #[test]
    fn test_advance_terminal_column_ignores_ansi_controls() {
        assert_eq!(advance_terminal_column(0, "\x1b[31mred\x1b[0m", None), 3);
        assert_eq!(
            advance_terminal_column(
                0,
                "\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\",
                None,
            ),
            4
        );
        assert_eq!(advance_terminal_column(3, "\x08!", None), 3);
    }

    #[test]
    fn test_use_output_polling() {
        let mut use_output = UseOutputImpl::default();
        assert_eq!(
            Pin::new(&mut use_output)
                .poll_change(&mut core::task::Context::from_waker(&noop_waker())),
            Poll::Pending
        );

        // Empty no-newline prints are true no-ops: they must not wake the
        // render loop or clear the retained live canvas. This guards against
        // using `print("")` as a repaint workaround, which would otherwise call
        // clear_terminal_output() during UseOutputState::exec().
        let mut no_op_output = UseOutputImpl::default();
        let no_op_stdout = no_op_output.use_stdout();
        let no_op_stderr = no_op_output.use_stderr();
        no_op_stdout.print("");
        no_op_stderr.print("");
        assert_eq!(
            Pin::new(&mut no_op_output)
                .poll_change(&mut core::task::Context::from_waker(&noop_waker())),
            Poll::Pending
        );

        let mut disabled_context_output = UseOutputImpl::default();
        let disabled_context_stdout = disabled_context_output.use_stdout();
        let disabled_context_canvas = Canvas::new(4, 1);
        assert_eq!(
            disabled_context_stdout.copy_selection_context_no_clear(
                &SelectionContext::disabled(),
                &disabled_context_canvas,
            ),
            ""
        );
        assert_eq!(
            Pin::new(&mut disabled_context_output)
                .poll_change(&mut core::task::Context::from_waker(&noop_waker())),
            Poll::Pending
        );

        let mut whitespace_output = UseOutputImpl::default();
        let whitespace_stdout = whitespace_output.use_stdout();
        let mut whitespace_canvas = Canvas::new(8, 1);
        whitespace_canvas.subview_mut(0, 0, 0, 0, 8, 1).set_text(
            0,
            0,
            "a   b",
            CanvasTextStyle::default(),
        );
        let mut whitespace_controller = SelectionController::new();
        whitespace_controller.selection_mut().start(1, 0);
        whitespace_controller.selection_mut().update(3, 0);
        whitespace_controller.selection_mut().finish();
        assert_eq!(
            whitespace_stdout.copy_on_select(&mut whitespace_controller, &whitespace_canvas),
            None
        );
        assert_eq!(
            Pin::new(&mut whitespace_output)
                .poll_change(&mut core::task::Context::from_waker(&noop_waker())),
            Poll::Pending,
            "whitespace-only copy-on-select should not queue an OSC 52 clipboard write"
        );

        let stdout = use_output.use_stdout();
        stdout.set_clipboard("copy");
        assert_eq!(
            Pin::new(&mut use_output)
                .poll_change(&mut core::task::Context::from_waker(&noop_waker())),
            Poll::Ready(())
        );

        let mut canvas = Canvas::new(8, 1);
        canvas
            .subview_mut(0, 0, 0, 0, 8, 1)
            .set_text(0, 0, "copy", CanvasTextStyle::default());
        let mut selection = SelectionState::new();
        selection.start(1, 0);
        selection.update(3, 0);
        assert_eq!(stdout.copy_selection_no_clear(&selection, &canvas), "opy");
        assert!(selection.has_selection());
        assert_eq!(stdout.copy_selection(&mut selection, &canvas), "opy");
        assert!(!selection.has_selection());

        let mut controller = SelectionController::new();
        controller.selection_mut().start(1, 0);
        controller.selection_mut().update(3, 0);
        controller.selection_mut().finish();
        assert_eq!(
            stdout.copy_on_select(&mut controller, &canvas).as_deref(),
            Some("opy")
        );
        assert_eq!(stdout.copy_on_select(&mut controller, &canvas), None);
        assert_eq!(
            Pin::new(&mut use_output)
                .poll_change(&mut core::task::Context::from_waker(&noop_waker())),
            Poll::Ready(())
        );

        stdout.println("Hello, world!");
        assert_eq!(
            Pin::new(&mut use_output)
                .poll_change(&mut core::task::Context::from_waker(&noop_waker())),
            Poll::Ready(())
        );

        let stderr = use_output.use_stderr();
        stderr.println("Hello, error!");
        assert_eq!(
            Pin::new(&mut use_output)
                .poll_change(&mut core::task::Context::from_waker(&noop_waker())),
            Poll::Ready(())
        );

        // Test print methods
        stdout.print("Hello, ");
        stdout.print("world!");
        stderr.print("Error: ");
        stderr.print("test");
        stderr.print("Warning: ");
        stderr.print("print test");
        assert_eq!(
            Pin::new(&mut use_output)
                .poll_change(&mut core::task::Context::from_waker(&noop_waker())),
            Poll::Ready(())
        );
    }

    #[component]
    fn MyComponent(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
        let mut system = hooks.use_context_mut::<SystemContext>();
        let (stdout, stderr) = hooks.use_output();
        stdout.println("Hello, world!");
        stderr.println("Hello, error!");
        stdout.print("Testing ");
        stdout.print("print ");
        stdout.println("method!");
        stderr.print("Error: ");
        stderr.println("test");
        stderr.print("Warning: ");
        stderr.println("print test");
        system.exit();
        element!(View)
    }

    #[apply(test!)]
    async fn test_use_output() {
        element!(MyComponent).render_loop().await.unwrap();
    }
    #[test]
    fn control_ack_is_pending_until_write_and_closes_when_owner_unmounts() {
        use futures::FutureExt;
        let mut owner = UseOutputImpl::default();
        let stdout = owner.use_stdout();
        let mut ack = stdout.write_control_sequence_and_wait("\x1b]52;c;QQ==\x07");
        assert!((&mut ack).now_or_never().is_none());
        assert!(!owner.state.lock().unwrap().queue[0].affects_visible_output());
        drop(owner);
        assert_eq!(
            futures::executor::block_on(ack).unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
        assert_eq!(
            futures::executor::block_on(stdout.write_control_sequence_and_wait("late"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::BrokenPipe
        );
    }

    #[cfg(feature = "unstable-output-streams")]
    #[derive(Clone)]
    struct AckProof {
        bytes: Arc<Mutex<Vec<u8>>>,
        fail_control: bool,
    }
    #[cfg(feature = "unstable-output-streams")]
    impl io::Write for AckProof {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.fail_control && bytes == b"\x1b]52;c;QQ==\x07" {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "controlled writer failure",
                ));
            }
            self.bytes.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[cfg(feature = "unstable-output-streams")]
    #[component]
    fn AckApp(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
        let proof = hooks.use_context::<AckProof>().clone();
        let (stdout, _) = hooks.use_output();
        let mut done = hooks.use_state(|| false);
        hooks.use_future(async move {
            let result = stdout
                .write_control_sequence_and_wait("\x1b]52;c;QQ==\x07")
                .await;
            if proof.fail_control {
                assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
            } else {
                result.unwrap();
                assert!(proof
                    .bytes
                    .lock()
                    .unwrap()
                    .windows(b"\x1b]52;c;QQ==\x07".len())
                    .any(|w| w == b"\x1b]52;c;QQ==\x07"));
            }
            done.set(true);
        });
        if done.get() {
            hooks.use_context_mut::<SystemContext>().exit();
        }
        element!(View)
    }

    #[cfg(feature = "unstable-output-streams")]
    #[apply(test!)]
    async fn control_ack_observes_real_writer_bytes_and_failures() {
        for fail_control in [false, true] {
            let proof = AckProof {
                bytes: Arc::new(Mutex::new(Vec::new())),
                fail_control,
            };
            element! {
                ContextProvider(value: crate::Context::owned(proof.clone())) { AckApp }
            }
            .render_loop()
            .stdout(proof)
            .await
            .unwrap();
        }
    }
    struct NoProcessBackend;
    impl crate::ClipboardBackend for NoProcessBackend {
        fn execute(
            &self,
            _: &str,
            _: &[&str],
            _: &str,
            _: std::time::Duration,
        ) -> BoxFuture<'static, i32> {
            panic!("explicit multiplexer override must not invoke native or tmux processes")
        }
        fn spawn(&self, _: BoxFuture<'static, ()>) {
            panic!("explicit override must not detach work")
        }
        fn is_kitty(&self) -> bool {
            true
        }
    }

    #[test]
    fn explicit_clipboard_wrapper_bypasses_provider_policy_and_fallback_remains_osc52() {
        let mut owner = UseOutputImpl::default();
        owner.clipboard = Some(Clipboard::new(Arc::new(NoProcessBackend)));
        let stdout = owner.use_stdout();
        stdout.set_clipboard_with_multiplexer("A", ClipboardMultiplexer::Screen);
        assert!(
            matches!(&owner.state.lock().unwrap().queue[0], Message::StdoutClipboard(text, ClipboardMultiplexer::Screen) if text == "A")
        );
        owner.clipboard = None;
        let plain = owner.use_stdout();
        assert_eq!(
            futures::executor::block_on(plain.prepare_clipboard("A")),
            "\x1b]52;c;QQ==\x07"
        );
        assert_eq!(plain.get_clipboard_path(), SelectionClipboardPath::Osc52);
    }

    #[component]
    fn ClipboardContextApp(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
        let (stdout, _) = hooks.use_output();
        assert!(
            stdout.clipboard.is_some(),
            "output hook must capture its nearest Clipboard provider"
        );
        stdout.set_clipboard_with_multiplexer("A", ClipboardMultiplexer::Screen);
        hooks.use_context_mut::<SystemContext>().exit();
        element!(View)
    }

    #[apply(test!)]
    async fn clipboard_context_is_inherited_by_the_real_output_hook() {
        element! {
            ContextProvider(value: crate::Context::owned(Clipboard::new(Arc::new(NoProcessBackend)))) {
                ClipboardContextApp
            }
        }.render_loop().await.unwrap();
    }
    #[test]
    fn clipboard_root_route_survives_child_drop_and_closes_with_root_without_cycles() {
        use futures::FutureExt;
        let mut root = UseOutputImpl::default();
        let root_stdout = root.use_stdout();
        let service = Clipboard::new(Arc::new(NoProcessBackend)).with_output(root_stdout);
        let mut child = UseOutputImpl::default();
        child.clipboard = Some(service);
        let child_stdout = child.use_stdout();
        // Rebinding a contextual handle strips its service; no recursive route.
        let rebound = Clipboard::new(Arc::new(NoProcessBackend)).with_output(child_stdout.clone());
        assert!(rebound.output().unwrap().clipboard.is_none());
        drop(child);
        let mut ack = child_stdout.write_control_sequence_and_wait("after child unmount");
        assert!((&mut ack).now_or_never().is_none());
        assert_eq!(root.state.lock().unwrap().queue.len(), 1);
        drop(root);
        assert_eq!(
            futures::executor::block_on(ack).unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
        assert_eq!(
            futures::executor::block_on(
                child_stdout.write_control_sequence_and_wait("after root unmount")
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::BrokenPipe
        );
    }

    #[cfg(feature = "unstable-output-streams")]
    #[derive(Default)]
    struct DeferredClipboardBackend {
        tasks: Mutex<Vec<BoxFuture<'static, ()>>>,
    }
    #[cfg(feature = "unstable-output-streams")]
    impl crate::ClipboardBackend for DeferredClipboardBackend {
        fn execute(
            &self,
            _: &str,
            _: &[&str],
            _: &str,
            _: std::time::Duration,
        ) -> BoxFuture<'static, i32> {
            Box::pin(async { 0 })
        }
        fn spawn(&self, task: BoxFuture<'static, ()>) {
            self.tasks.lock().unwrap().push(task);
        }
        fn is_kitty(&self) -> bool {
            true
        }
    }
    #[cfg(feature = "unstable-output-streams")]
    #[derive(Clone)]
    struct LifetimeProof {
        output: AckProof,
        backend: Arc<DeferredClipboardBackend>,
        child_unmounted: Arc<std::sync::atomic::AtomicBool>,
    }
    #[cfg(feature = "unstable-output-streams")]
    #[derive(Clone)]
    struct ChildControl {
        visible: State<bool>,
        unmounted: Arc<std::sync::atomic::AtomicBool>,
    }
    #[cfg(feature = "unstable-output-streams")]
    struct MarkChildUnmount(Arc<std::sync::atomic::AtomicBool>);
    #[cfg(feature = "unstable-output-streams")]
    impl Hook for MarkChildUnmount {}
    #[cfg(feature = "unstable-output-streams")]
    impl Drop for MarkChildUnmount {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
    #[cfg(feature = "unstable-output-streams")]
    #[component]
    fn ClipboardTransientChild(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
        let control = hooks.use_context::<ChildControl>().clone();
        let unmounted = control.unmounted.clone();
        hooks.use_hook(move || MarkChildUnmount(unmounted));
        let (stdout, _) = hooks.use_output();
        hooks.use_future(async move {
            stdout.set_clipboard("A");
            let mut visible = control.visible;
            visible.set(false);
        });
        element!(View)
    }
    #[cfg(feature = "unstable-output-streams")]
    #[component]
    fn ClipboardRetainedRoot(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
        let proof = hooks.use_context::<LifetimeProof>().clone();
        let (stdout, _) = hooks.use_output();
        let clipboard = hooks
            .use_state(|| Clipboard::new(proof.backend.clone()).with_output(stdout))
            .read()
            .clone();
        let visible = hooks.use_state(|| true);
        let mut done = hooks.use_state(|| false);
        let child_control = ChildControl {
            visible,
            unmounted: proof.child_unmounted.clone(),
        };
        hooks.use_future(async move {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !proof
                .child_unmounted
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                assert!(
                    std::time::Instant::now() < deadline,
                    "child did not unmount"
                );
                smol::Timer::after(std::time::Duration::from_millis(1)).await;
            }
            // Delay the detached clipboard continuation until the actual child
            // output hook has been destroyed; the retained root still owns I/O.
            let tasks = std::mem::take(&mut *proof.backend.tasks.lock().unwrap());
            assert!(!tasks.is_empty());
            for task in tasks {
                task.await;
            }
            assert!(proof
                .output
                .bytes
                .lock()
                .unwrap()
                .windows(b"52;c;QQ==".len())
                .any(|w| w == b"52;c;QQ=="));
            done.set(true);
        });
        if done.get() {
            hooks.use_context_mut::<SystemContext>().exit();
        }
        element! {
            ContextProvider(value: crate::Context::owned(clipboard)) {
                ContextProvider(value: crate::Context::owned(child_control)) {
                    #(if visible.get() { Some(element!(ClipboardTransientChild)) } else { None })
                }
            }
        }
    }
    #[cfg(feature = "unstable-output-streams")]
    #[apply(test!)]
    async fn detached_clipboard_output_reaches_real_writer_after_child_unmount() {
        let output = AckProof {
            bytes: Arc::new(Mutex::new(Vec::new())),
            fail_control: false,
        };
        let proof = LifetimeProof {
            output: output.clone(),
            backend: Arc::new(DeferredClipboardBackend::default()),
            child_unmounted: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        element! {
            ContextProvider(value: crate::Context::owned(proof)) { ClipboardRetainedRoot }
        }
        .render_loop()
        .stdout(output)
        .await
        .unwrap();
    }
}

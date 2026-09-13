//! Clipboard policy from CC Ink `termio/osc.ts:62-231`.
//! The application supplies process execution and detached-task lifetime;
//! iocraft owns native fallback, tmux policy, caching and terminal sequences.

use crate::ansi::{self, MultiplexerPassthrough};
use crate::hooks::{SelectionClipboardPath, StdoutHandle};
use futures::future::BoxFuture;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Host capabilities needed by the clipboard service, with no executor dependency.
pub trait ClipboardBackend: Send + Sync + 'static {
    /// Starts the process immediately, returning its eventual exit status.
    /// The process must continue if the returned future is dropped.
    fn execute(
        &self,
        program: &str,
        args: &[&str],
        input: &str,
        timeout: Duration,
    ) -> BoxFuture<'static, i32>;
    /// Detaches a continuation from the component lifetime.
    fn spawn(&self, future: BoxFuture<'static, ()>);
    /// Uses the application's resolved terminal identity.
    fn is_kitty(&self) -> bool;
}

/// Shared clipboard service. Provide one through a `ContextProvider` so all
/// `use_output` handles share the source module's Linux-tool cache.
#[derive(Clone)]
pub struct Clipboard {
    backend: Arc<dyn ClipboardBackend>,
    linux_copy: Arc<Mutex<LinuxCopy>>,
    output: Option<Arc<StdoutHandle>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinuxCopy {
    Unprobed,
    Unavailable,
    WlCopy,
    Xclip,
    Xsel,
}

struct Environment {
    platform: &'static str,
    ssh: bool,
    tmux: bool,
    iterm: bool,
}
impl Environment {
    fn current() -> Self {
        let nonempty = |name| std::env::var_os(name).is_some_and(|v| !v.is_empty());
        Self {
            platform: std::env::consts::OS,
            ssh: nonempty("SSH_CONNECTION"),
            tmux: nonempty("TMUX"),
            iterm: std::env::var("LC_TERMINAL").as_deref() == Ok("iTerm2"),
        }
    }
    fn get_clipboard_path(&self) -> SelectionClipboardPath {
        if self.platform == "macos" && !self.ssh {
            SelectionClipboardPath::Native
        } else if self.tmux {
            SelectionClipboardPath::TmuxBuffer
        } else {
            SelectionClipboardPath::Osc52
        }
    }
}

impl Clipboard {
    /// Creates a service whose clones share the native Linux-tool cache.
    pub fn new(backend: Arc<dyn ClipboardBackend>) -> Self {
        Self {
            backend,
            linux_copy: Arc::new(Mutex::new(LinuxCopy::Unprobed)),
            output: None,
        }
    }

    /// Binds clipboard control output to a retained root's existing queue.
    /// The handle's clipboard context is stripped so binding a contextual
    /// handle cannot create a recursive service/output route.
    pub fn with_output(mut self, output: StdoutHandle) -> Self {
        self.output = Some(Arc::new(output.without_clipboard()));
        self
    }

    pub(crate) fn output(&self) -> Option<&StdoutHandle> {
        self.output.as_deref()
    }

    /// Maps to CC `getClipboardPath`: confidence for the notification, not a
    /// promise that the terminal accepted the clipboard sequence.
    pub fn get_clipboard_path(&self) -> SelectionClipboardPath {
        Environment::current().get_clipboard_path()
    }

    /// Maps to CC `setClipboard`. Starts native and tmux work immediately;
    /// only tmux is awaited before returning the raw control sequence.
    pub fn set_clipboard(&self, text: &str) -> BoxFuture<'static, String> {
        self.set_clipboard_with_environment(text, Environment::current())
    }

    pub(crate) fn spawn(&self, future: BoxFuture<'static, ()>) {
        self.backend.spawn(future);
    }

    fn set_clipboard_with_environment(
        &self,
        text: &str,
        env: Environment,
    ) -> BoxFuture<'static, String> {
        let raw = ansi::osc52_clipboard_sequence_with_kitty(text, self.backend.is_kitty());
        if !env.ssh {
            self.copy_native(text, env.platform);
        }
        let tmux = self.tmux_load_buffer(text, &env);
        let passthrough =
            ansi::osc52_clipboard_sequence_for_multiplexer(text, MultiplexerPassthrough::Tmux);
        Box::pin(async move {
            if tmux.await {
                passthrough
            } else {
                raw
            }
        })
    }

    // Maps to CC `tmuxLoadBuffer`: iTerm2 must not receive tmux's -w emission.
    fn tmux_load_buffer(&self, text: &str, env: &Environment) -> BoxFuture<'static, bool> {
        let process = env.tmux.then(|| {
            self.backend.execute(
                "tmux",
                if env.iterm {
                    &["load-buffer", "-"]
                } else {
                    &["load-buffer", "-w", "-"]
                },
                text,
                Duration::from_millis(2000),
            )
        });
        Box::pin(async move {
            match process {
                Some(p) => p.await == 0,
                None => false,
            }
        })
    }

    // Maps to CC `copyNative`: a cached failure never triggers a new probe.
    fn copy_native(&self, text: &str, platform: &str) {
        let timeout = Duration::from_millis(2000);
        let cached = *self.linux_copy.lock().unwrap();
        let command: Option<(&str, &[&str])> = match platform {
            "macos" => Some(("pbcopy", &[])),
            "windows" => Some(("clip", &[])),
            "linux" => match cached {
                LinuxCopy::Unavailable | LinuxCopy::Unprobed => None,
                LinuxCopy::WlCopy => Some(("wl-copy", &[])),
                LinuxCopy::Xclip => Some(("xclip", &["-selection", "clipboard"])),
                LinuxCopy::Xsel => Some(("xsel", &["--clipboard", "--input"])),
            },
            _ => None,
        };
        if let Some((program, args)) = command {
            drop(self.backend.execute(program, args, text, timeout));
        } else if platform == "linux" && cached == LinuxCopy::Unprobed {
            // No probing state: simultaneous first calls independently launch
            // wl-copy, exactly as the source's undefined/null/tool union does.
            let first = self.backend.execute("wl-copy", &[], text, timeout);
            let service = self.clone();
            let text = text.to_string();
            self.backend.spawn(Box::pin(async move {
                let winner = if first.await == 0 {
                    LinuxCopy::WlCopy
                } else if service
                    .backend
                    .execute("xclip", &["-selection", "clipboard"], &text, timeout)
                    .await
                    == 0
                {
                    LinuxCopy::Xclip
                } else if service
                    .backend
                    .execute("xsel", &["--clipboard", "--input"], &text, timeout)
                    .await
                    == 0
                {
                    LinuxCopy::Xsel
                } else {
                    LinuxCopy::Unavailable
                };
                *service.linux_copy.lock().unwrap() = winner;
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{channel::oneshot, executor::block_on};
    use std::collections::VecDeque;

    #[derive(Default)]
    struct Backend {
        calls: Mutex<Vec<(String, Vec<String>, String, Duration)>>,
        replies: Mutex<VecDeque<BoxFuture<'static, i32>>>,
        tasks: Mutex<Vec<BoxFuture<'static, ()>>>,
    }
    impl ClipboardBackend for Backend {
        fn execute(
            &self,
            program: &str,
            args: &[&str],
            input: &str,
            timeout: Duration,
        ) -> BoxFuture<'static, i32> {
            self.calls.lock().unwrap().push((
                program.into(),
                args.iter().map(|s| s.to_string()).collect(),
                input.into(),
                timeout,
            ));
            self.replies
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Box::pin(async { 0 }))
        }
        fn spawn(&self, future: BoxFuture<'static, ()>) {
            self.tasks.lock().unwrap().push(future);
        }
        fn is_kitty(&self) -> bool {
            true
        }
    }
    impl Backend {
        fn reply(&self, code: i32) {
            self.replies
                .lock()
                .unwrap()
                .push_back(Box::pin(async move { code }));
        }
        fn drain(&self) {
            let tasks = std::mem::take(&mut *self.tasks.lock().unwrap());
            for task in tasks {
                block_on(task);
            }
        }
        fn programs(&self) -> Vec<String> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .map(|c| c.0.clone())
                .collect()
        }
    }
    fn env(platform: &'static str, ssh: bool, tmux: bool, iterm: bool) -> Environment {
        Environment {
            platform,
            ssh,
            tmux,
            iterm,
        }
    }

    #[test]
    fn clipboard_native_starts_before_tmux_and_does_not_wait_for_native_completion() {
        let backend = Arc::new(Backend::default());
        backend
            .replies
            .lock()
            .unwrap()
            .push_back(Box::pin(futures::future::pending()));
        let (release, reply) = oneshot::channel();
        backend
            .replies
            .lock()
            .unwrap()
            .push_back(Box::pin(async move { reply.await.unwrap() }));
        let service = Clipboard::new(backend.clone());
        let prepared =
            service.set_clipboard_with_environment("中文\n🌈", env("macos", false, true, false));
        assert_eq!(backend.programs(), ["pbcopy", "tmux"]);
        let calls = backend.calls.lock().unwrap();
        assert_eq!(calls[1].1, ["load-buffer", "-w", "-"]);
        assert!(calls
            .iter()
            .all(|c| c.2 == "中文\n🌈" && c.3 == Duration::from_secs(2)));
        drop(calls);
        release.send(0).unwrap();
        assert_eq!(
            block_on(prepared),
            "\x1bPtmux;\x1b\x1b]52;c;5Lit5paHCvCfjIg=\x07\x1b\\"
        );
    }

    #[test]
    fn clipboard_iterm_failure_returns_raw_kitty_sequence_and_ssh_skips_native() {
        let backend = Arc::new(Backend::default());
        backend.reply(1);
        let service = Clipboard::new(backend.clone());
        assert_eq!(
            block_on(service.set_clipboard_with_environment("A", env("macos", true, true, true))),
            "\x1b]52;c;QQ==\x1b\\"
        );
        assert_eq!(backend.programs(), ["tmux"]);
        assert_eq!(backend.calls.lock().unwrap()[0].1, ["load-buffer", "-"]);
        assert_eq!(
            block_on(service.set_clipboard_with_environment("", env("macos", true, false, false))),
            "\x1b]52;c;\x1b\\"
        );
        assert_eq!(backend.programs(), ["tmux"]);
        assert_eq!(
            env("macos", false, true, false).get_clipboard_path(),
            SelectionClipboardPath::Native
        );
        assert_eq!(
            env("linux", false, false, false).get_clipboard_path(),
            SelectionClipboardPath::Osc52
        );
        assert_eq!(
            env("macos", true, true, false).get_clipboard_path(),
            SelectionClipboardPath::TmuxBuffer
        );
    }

    #[test]
    fn clipboard_linux_probe_order_cached_success_failure_and_concurrent_first_calls() {
        for (replies, expected, winner) in [
            (vec![0], vec!["wl-copy"], LinuxCopy::WlCopy),
            (vec![1, 0], vec!["wl-copy", "xclip"], LinuxCopy::Xclip),
            (
                vec![1, 1, 0],
                vec!["wl-copy", "xclip", "xsel"],
                LinuxCopy::Xsel,
            ),
            (
                vec![1, 1, 1],
                vec!["wl-copy", "xclip", "xsel"],
                LinuxCopy::Unavailable,
            ),
        ] {
            let backend = Arc::new(Backend::default());
            for reply in replies {
                backend.reply(reply);
            }
            let service = Clipboard::new(backend.clone());
            drop(service.set_clipboard_with_environment("one", env("linux", false, false, false)));
            backend.drain();
            assert_eq!(backend.programs(), expected);
            assert_eq!(*service.linux_copy.lock().unwrap(), winner);
            let count = backend.calls.lock().unwrap().len();
            backend.reply(1);
            drop(
                service
                    .clone()
                    .set_clipboard_with_environment("two", env("linux", false, false, false)),
            );
            backend.drain();
            assert_eq!(
                backend.calls.lock().unwrap().len(),
                count + usize::from(winner != LinuxCopy::Unavailable)
            );
            assert_eq!(*service.linux_copy.lock().unwrap(), winner);
        }
        let backend = Arc::new(Backend::default());
        let service = Clipboard::new(backend.clone());
        drop(service.set_clipboard_with_environment("one", env("linux", false, false, false)));
        drop(
            service
                .clone()
                .set_clipboard_with_environment("two", env("linux", false, false, false)),
        );
        assert_eq!(backend.programs(), ["wl-copy", "wl-copy"]);
        assert_eq!(*service.linux_copy.lock().unwrap(), LinuxCopy::Unprobed);
        backend.drain();
        assert_eq!(*service.linux_copy.lock().unwrap(), LinuxCopy::WlCopy);
    }

    #[test]
    fn clipboard_windows_uses_clip_without_shell_or_arguments() {
        let backend = Arc::new(Backend::default());
        let service = Clipboard::new(backend.clone());
        drop(service.set_clipboard_with_environment("text", env("windows", false, false, false)));
        assert_eq!(backend.programs(), ["clip"]);
        assert!(backend.calls.lock().unwrap()[0].1.is_empty());
    }
}

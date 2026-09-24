//! Diagnostic switches read once from the process environment.
//!
//! Two variables, each a comma-separated list of tokens. Tokens are trimmed
//! and matched case-insensitively:
//!
//! - `IOCRAFT_DISABLE` turns off an optimization that is on by default, so a
//!   rendering artifact can be bisected back to the pass that introduced it.
//!   `push-wake`: poll every component every frame instead of only the woken
//!   paths. `retained-blit`: re-draw memo-retained subtrees instead of
//!   blitting them from the previous canvas. `damage-skip`: diff every row
//!   instead of only the rows a canvas write marked.
//! - `IOCRAFT_DEBUG` turns on a diagnostic that is off by default because it
//!   costs per frame. `settle`: print each component whose render-phase state
//!   write re-ran the update. `cells`: count changed cells per frame for the
//!   render profile. `frame-log=PATH`: append one line per frame describing
//!   the canvas and the repaint. `layout-dump=PATH`: append every frame's
//!   layout tree.
//!
//! Unknown tokens are ignored, so a misspelling cannot enable a different
//! switch. A `PATH` is taken verbatim after the first `=`, so it keeps its
//! case and cannot contain a comma. Both variables are read once per process.
//! Tests never set them: each gate keeps a test-only override beside it
//! (`set_push_wake_for_tests` and friends) so parallel tests do not race on a
//! process-wide value.

use std::path::PathBuf;
use std::sync::OnceLock;

/// Optimizations turned off by `IOCRAFT_DISABLE`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Disabled {
    pub(crate) push_wake: bool,
    pub(crate) retained_blit: bool,
    pub(crate) damage_skip: bool,
}

impl Disabled {
    fn parse(value: Option<&str>) -> Self {
        let mut disabled = Self::default();
        for token in tokens(value) {
            match token.to_ascii_lowercase().as_str() {
                "push-wake" => disabled.push_wake = true,
                "retained-blit" => disabled.retained_blit = true,
                "damage-skip" => disabled.damage_skip = true,
                _ => {}
            }
        }
        disabled
    }
}

/// Diagnostics turned on by `IOCRAFT_DEBUG`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Diagnostics {
    pub(crate) settle: bool,
    pub(crate) cells: bool,
    pub(crate) frame_log: Option<PathBuf>,
    pub(crate) layout_dump: Option<PathBuf>,
}

impl Diagnostics {
    fn parse(value: Option<&str>) -> Self {
        let mut diagnostics = Self::default();
        for token in tokens(value) {
            let (key, path) = match token.split_once('=') {
                Some((key, path)) => (key.trim(), Some(path.trim())),
                None => (token, None),
            };
            match key.to_ascii_lowercase().as_str() {
                "settle" => diagnostics.settle = true,
                "cells" => diagnostics.cells = true,
                "frame-log" => diagnostics.frame_log = non_empty_path(path),
                "layout-dump" => diagnostics.layout_dump = non_empty_path(path),
                _ => {}
            }
        }
        diagnostics
    }
}

fn tokens(value: Option<&str>) -> impl Iterator<Item = &str> {
    value
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
}

fn non_empty_path(path: Option<&str>) -> Option<PathBuf> {
    path.filter(|path| !path.is_empty()).map(PathBuf::from)
}

/// The `IOCRAFT_DISABLE` list, parsed on first use.
pub(crate) fn disabled() -> &'static Disabled {
    static DISABLED: OnceLock<Disabled> = OnceLock::new();
    DISABLED.get_or_init(|| Disabled::parse(std::env::var("IOCRAFT_DISABLE").ok().as_deref()))
}

/// The `IOCRAFT_DEBUG` list, parsed on first use.
pub(crate) fn diagnostics() -> &'static Diagnostics {
    static DIAGNOSTICS: OnceLock<Diagnostics> = OnceLock::new();
    DIAGNOSTICS.get_or_init(|| Diagnostics::parse(std::env::var("IOCRAFT_DEBUG").ok().as_deref()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_or_empty_lists_change_nothing() {
        assert_eq!(Disabled::parse(None), Disabled::default());
        assert_eq!(Disabled::parse(Some("")), Disabled::default());
        assert_eq!(Diagnostics::parse(None), Diagnostics::default());
        assert_eq!(Diagnostics::parse(Some(" , ")), Diagnostics::default());
    }

    #[test]
    fn disable_tokens_are_trimmed_and_case_insensitive() {
        let disabled = Disabled::parse(Some(" Push-Wake ,DAMAGE-SKIP"));
        assert_eq!(
            disabled,
            Disabled {
                push_wake: true,
                retained_blit: false,
                damage_skip: true,
            }
        );
    }

    #[test]
    fn unknown_and_legacy_tokens_are_ignored() {
        // The retired per-feature variables took `0` / `1`; those values must
        // not flip anything under the list syntax.
        assert_eq!(Disabled::parse(Some("0")), Disabled::default());
        assert_eq!(
            Disabled::parse(Some("1,pushwake,retained_blit")),
            Disabled::default()
        );
        assert_eq!(
            Diagnostics::parse(Some("1,true,frame_log=/tmp/x")),
            Diagnostics::default()
        );
    }

    #[test]
    fn debug_paths_keep_their_case_and_need_a_value() {
        let diagnostics =
            Diagnostics::parse(Some("Settle,frame-log=/Tmp/Frames.log,layout-dump=,cells"));
        assert_eq!(
            diagnostics,
            Diagnostics {
                settle: true,
                cells: true,
                frame_log: Some(PathBuf::from("/Tmp/Frames.log")),
                layout_dump: None,
            }
        );
    }

    #[test]
    fn a_later_path_token_replaces_an_earlier_one() {
        let diagnostics = Diagnostics::parse(Some("frame-log=a, frame-log = b "));
        assert_eq!(diagnostics.frame_log, Some(PathBuf::from("b")));
    }
}

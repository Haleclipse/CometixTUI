use crate::style::Color;
use crossterm::{csi, style::Attribute};
use std::{
    env,
    io::{self, IsTerminal, Write},
    sync::OnceLock,
};

pub(crate) fn sgr_reset(w: &mut impl Write) -> io::Result<()> {
    w.write_all(csi!("0m").as_bytes())
}

pub(crate) fn erase_to_eol(w: &mut impl Write) -> io::Result<()> {
    w.write_all(csi!("K").as_bytes())
}

pub(crate) fn sgr_attr(w: &mut impl Write, attr: Attribute) -> io::Result<()> {
    write!(w, csi!("{}m"), attr.sgr())
}

/// The SGR channel a color is encoded for: foreground (38), background (48),
/// or underline color (58).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ColorChannel {
    Foreground,
    Background,
    Underline,
}

impl ColorChannel {
    /// The chalk close code restoring the channel's default (39/49/59).
    fn close_code(self) -> u8 {
        match self {
            Self::Foreground => 39,
            Self::Background => 49,
            Self::Underline => 59,
        }
    }
}

// Per-thread level override for tests. The production singleton path is
// unreachable under `cargo test` (stdout is a pipe → full fidelity), so
// level-dependent behavior is exercised through this hook instead; being
// thread-local, concurrent tests cannot race each other.
#[cfg(test)]
thread_local! {
    pub(crate) static TEST_COLOR_LEVEL: std::cell::Cell<Option<chalk::ColorLevel>> =
        const { std::cell::Cell::new(None) };
}

/// Pins the per-thread color level for a test and restores detection on drop,
/// so a panicking assertion cannot leak the override into reused threads.
#[cfg(test)]
pub(crate) struct TestColorLevelGuard;

#[cfg(test)]
impl TestColorLevelGuard {
    pub(crate) fn pin(level: chalk::ColorLevel) -> Self {
        TEST_COLOR_LEVEL.with(|cell| cell.set(Some(level)));
        TestColorLevelGuard
    }
}

#[cfg(test)]
impl Drop for TestColorLevelGuard {
    fn drop(&mut self) {
        TEST_COLOR_LEVEL.with(|cell| cell.set(None));
    }
}

/// The color level canvas SGR output is encoded at. On a terminal this reads
/// chalk's stdout singleton on every call — as in ink, where a runtime write
/// to `chalk.level` (the primitive CC's boost/clamp uses) affects all
/// subsequent output. Only stdout's tty nature is cached: it cannot change
/// mid-process, and skipping the isatty syscall keeps per-transition encoding
/// cheap. Off-terminal output (tests, pipes, render-to-string) stays at full
/// fidelity: a canvas is a structured intermediate representation, and
/// downgrading is a property of the terminal actually attached.
pub(crate) fn render_color_level() -> chalk::ColorLevel {
    #[cfg(test)]
    if let Some(level) = TEST_COLOR_LEVEL.with(|cell| cell.get()) {
        return level;
    }
    static STDOUT_IS_TTY: OnceLock<bool> = OnceLock::new();
    if *STDOUT_IS_TTY.get_or_init(|| io::stdout().is_terminal()) {
        chalk::stdout_level()
    } else {
        3
    }
}

/// Whether any SGR styling is emitted at all. At chalk level 0 (FORCE_COLOR=0,
/// TERM=dumb) CC's colorize.ts is a full passthrough — no colors, no bold, no
/// dim, zero SGR bytes. Layout control sequences (erase-to-eol, cursor motion)
/// and OSC 8 hyperlinks are not styling and stay unaffected.
pub(crate) fn styles_enabled() -> bool {
    render_color_level() != 0
}

pub(crate) fn sgr_fg(w: &mut impl Write, color: Color) -> io::Result<()> {
    sgr_color(w, color, ColorChannel::Foreground, render_color_level())
}

pub(crate) fn sgr_bg(w: &mut impl Write, color: Color) -> io::Result<()> {
    sgr_color(w, color, ColorChannel::Background, render_color_level())
}

pub(crate) fn sgr_underline_color(w: &mut impl Write, color: Color) -> io::Result<()> {
    sgr_color(w, color, ColorChannel::Underline, render_color_level())
}

/// Maps to ink colorize.ts: every structured color is encoded through chalk's
/// model selection (chalk/index.js `getModelAnsi`), so output downgrades on
/// 256-color and 16-color terminals exactly like CC. Level 0 makes this a
/// no-op — the color half of colorize.ts's full passthrough; the attribute
/// half is gated at the transition writers via [`styles_enabled`]. (NO_COLOR
/// itself is ignored by chalk v6; the "colors only" NO_COLOR behavior is
/// crossterm's `Colored::Display`, which this encoder deliberately replaces.)
fn sgr_color(
    w: &mut impl Write,
    color: Color,
    channel: ColorChannel,
    level: chalk::ColorLevel,
) -> io::Result<()> {
    if level == 0 {
        return Ok(());
    }
    match color {
        Color::Reset => write!(w, csi!("{}m"), channel.close_code()),
        Color::Rgb { r, g, b } => match level {
            3 => match channel {
                ColorChannel::Foreground => write!(w, csi!("38;2;{};{};{}m"), r, g, b),
                ColorChannel::Background => write!(w, csi!("48;2;{};{};{}m"), r, g, b),
                ColorChannel::Underline => write!(w, csi!("58;2;{};{};{}m"), r, g, b),
            },
            2 => write_ansi256(w, chalk::rgb_to_ansi256(r, g, b), channel),
            _ => write_ansi16(w, chalk::rgb_to_ansi(r, g, b), channel),
        },
        Color::AnsiValue(code) => {
            if level >= 2 {
                write_ansi256(w, code, channel)
            } else {
                write_ansi16(w, chalk::ansi256_to_ansi(code), channel)
            }
        }
        named => match ansi16_code(named) {
            Some(code) => write_ansi16(w, code, channel),
            None => Ok(()),
        },
    }
}

/// crossterm named colors as chalk/ansi-styles foreground open codes. The
/// un-prefixed crossterm colors are the bright variants (its own 256-color
/// mapping uses indices 9-15), so `Red` → 91 (redBright), `DarkRed` → 31 (red).
fn ansi16_code(color: Color) -> Option<u8> {
    Some(match color {
        Color::Black => 30,
        Color::DarkRed => 31,
        Color::DarkGreen => 32,
        Color::DarkYellow => 33,
        Color::DarkBlue => 34,
        Color::DarkMagenta => 35,
        Color::DarkCyan => 36,
        Color::Grey => 37,
        Color::DarkGrey => 90,
        Color::Red => 91,
        Color::Green => 92,
        Color::Yellow => 93,
        Color::Blue => 94,
        Color::Magenta => 95,
        Color::Cyan => 96,
        Color::White => 97,
        _ => return None,
    })
}

/// ansi-styles `wrapAnsi256`, plus chalk v6's 58;5 underline-color form.
fn write_ansi256(w: &mut impl Write, code: u8, channel: ColorChannel) -> io::Result<()> {
    match channel {
        ColorChannel::Foreground => write!(w, csi!("38;5;{}m"), code),
        ColorChannel::Background => write!(w, csi!("48;5;{}m"), code),
        ColorChannel::Underline => write!(w, csi!("58;5;{}m"), code),
    }
}

/// ansi-styles `wrapAnsi16` over a 30-37/90-97 base code. SGR 58 has no basic
/// 16-color form, so chalk v6's `wrapUnderlineAnsi` maps the code onto its
/// palette index instead.
fn write_ansi16(w: &mut impl Write, code: u8, channel: ColorChannel) -> io::Result<()> {
    match channel {
        ColorChannel::Foreground => write!(w, csi!("{}m"), code),
        ColorChannel::Background => write!(w, csi!("{}m"), code + 10),
        ColorChannel::Underline => {
            let palette = if code < 90 { code - 30 } else { code - 90 + 8 };
            write!(w, csi!("58;5;{}m"), palette)
        }
    }
}

fn sanitize_hyperlink_href(href: &str) -> String {
    href.chars().filter(|ch| !ch.is_control()).collect()
}

fn base36_u32(mut value: u32) -> String {
    if value == 0 {
        return "0".to_string();
    }
    let mut out = Vec::new();
    while value > 0 {
        let digit = (value % 36) as u8;
        out.push(match digit {
            0..=9 => b'0' + digit,
            _ => b'a' + (digit - 10),
        });
        value /= 36;
    }
    out.reverse();
    String::from_utf8(out).expect("base36 digits are ascii")
}

fn osc8_id(url: &str) -> String {
    // Mirrors CC Ink's termio/osc.ts `osc8Id`: JS bitwise math keeps a signed
    // 32-bit accumulator, then `>>> 0` formats it as unsigned base36.
    let mut hash: i32 = 0;
    for ch in url.chars() {
        hash = hash
            .wrapping_shl(5)
            .wrapping_sub(hash)
            .wrapping_add(ch as i32);
    }
    base36_u32(hash as u32)
}

const ADDITIONAL_HYPERLINK_TERMINALS: &[&str] = &[
    "ghostty",
    "Hyper",
    "kitty",
    "alacritty",
    "iTerm.app",
    "iTerm2",
];

pub(crate) fn supports_hyperlinks_with_env(
    mut env_lookup: impl FnMut(&str) -> Option<String>,
    stdout_supported: bool,
) -> bool {
    // Mirrors CC Ink's supports-hyperlinks.ts wrapper: trust the base
    // supports-hyperlinks result first, then allowlist terminals that are known
    // to support OSC 8 but may not be detected by the library.
    if stdout_supported {
        return true;
    }

    if env_lookup("TERM_PROGRAM")
        .is_some_and(|term| ADDITIONAL_HYPERLINK_TERMINALS.contains(&term.as_str()))
    {
        return true;
    }

    if env_lookup("LC_TERMINAL")
        .is_some_and(|term| ADDITIONAL_HYPERLINK_TERMINALS.contains(&term.as_str()))
    {
        return true;
    }

    if env_lookup("TERM").is_some_and(|term| term.contains("kitty")) {
        return true;
    }

    false
}

fn supports_hyperlinks_base_stdout() -> bool {
    if env::var("FORCE_HYPERLINK").is_ok_and(|value| value != "0") {
        return true;
    }
    if env::var("NETLIFY").is_ok_and(|value| !value.is_empty()) {
        return true;
    }
    std::io::stdout().is_terminal()
        && (env::var_os("WT_SESSION").is_some()
            || env::var("TERM_PROGRAM").is_ok_and(|term| {
                matches!(
                    term.as_str(),
                    "iTerm.app" | "WezTerm" | "vscode" | "ghostty" | "zed"
                )
            })
            || env::var("TERM")
                .is_ok_and(|term| matches!(term.as_str(), "alacritty" | "xterm-kitty"))
            || env::var("VTE_VERSION")
                .ok()
                .and_then(|value| value.parse::<u32>().ok())
                .is_some_and(|version| version >= 5000))
}

/// Returns whether stdout should emit OSC 8 hyperlink metadata.
///
/// This mirrors CC Ink's `supportsHyperlinks(...)` helper: it trusts the
/// upstream supports-hyperlinks stdout probe and extends it with the fork's
/// additional terminal allowlist (`ghostty`, Hyper, kitty, Alacritty, iTerm2,
/// LC_TERMINAL preservation through tmux, and `TERM=xterm-kitty`).
pub fn supports_hyperlinks() -> bool {
    supports_hyperlinks_with_env(|key| env::var(key).ok(), supports_hyperlinks_base_stdout())
}

pub(crate) fn hyperlink_open(w: &mut impl Write, href: &str) -> io::Result<()> {
    let href = sanitize_hyperlink_href(href);
    let id = osc8_id(&href);
    write!(w, "\x1b]8;id={id};{href}\x1b\\")
}

pub(crate) fn hyperlink_close(w: &mut impl Write) -> io::Result<()> {
    w.write_all(b"\x1b]8;;\x1b\\")
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0b0000_0011) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(((b1 & 0b0000_1111) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(b2 & 0b0011_1111) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Terminal multiplexer passthrough wrapper to use for escape sequences.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MultiplexerPassthrough {
    /// Do not wrap the sequence.
    None,
    /// Wrap for tmux DCS passthrough, doubling inner ESC bytes.
    Tmux,
    /// Wrap for GNU screen passthrough.
    Screen,
}

/// Wraps a terminal escape sequence for multiplexer passthrough.
///
/// Mirrors CC Ink's `wrapForMultiplexer(...)` / `tmuxPassthrough(...)`:
/// tmux requires inner ESC bytes to be doubled, while GNU screen wraps the
/// payload as-is. BEL is intentionally not escaped.
pub(crate) fn wrap_for_multiplexer_sequence(
    sequence: &str,
    multiplexer: MultiplexerPassthrough,
) -> String {
    match multiplexer {
        MultiplexerPassthrough::None => sequence.to_string(),
        MultiplexerPassthrough::Tmux => {
            let escaped = sequence.replace('\x1b', "\x1b\x1b");
            format!("\x1bPtmux;{escaped}\x1b\\")
        }
        MultiplexerPassthrough::Screen => format!("\x1bP{sequence}\x1b\\"),
    }
}

/// Detects the current terminal multiplexer passthrough mode from environment.
///
/// This mirrors CC Ink's `wrapForMultiplexer(...)`: tmux and GNU screen need
/// DCS passthrough for OSC notification/progress sequences to reach the outer
/// terminal, while raw BEL must remain unwrapped so tmux can use it as a bell.
pub(crate) fn current_multiplexer_passthrough() -> MultiplexerPassthrough {
    if env::var_os("TMUX").is_some_and(|v| !v.is_empty()) {
        MultiplexerPassthrough::Tmux
    } else if env::var_os("STY").is_some_and(|v| !v.is_empty()) {
        MultiplexerPassthrough::Screen
    } else {
        MultiplexerPassthrough::None
    }
}

/// Wraps a terminal escape sequence for the current multiplexer environment.
pub(crate) fn wrap_for_current_multiplexer_sequence(sequence: &str) -> String {
    wrap_for_multiplexer_sequence(sequence, current_multiplexer_passthrough())
}

/// Builds an OSC sequence using the same terminator policy as CC Ink: Kitty
/// receives ST to avoid audible bells; other terminals use BEL.
pub(crate) fn osc_sequence(parts: &[String]) -> String {
    let terminator = if env::var_os("KITTY_WINDOW_ID").is_some()
        || env::var("TERM").is_ok_and(|term| term.contains("kitty"))
    {
        "\x1b\\"
    } else {
        "\x07"
    };
    format!("\x1b]{}{}", parts.join(";"), terminator)
}

/// Filters OSC payload text so user-provided notification strings cannot
/// terminate the sequence and inject terminal controls.
pub(crate) fn sanitize_osc_payload(text: &str) -> String {
    text.chars()
        .filter(|ch| matches!(ch, '\n' | '\t') || !ch.is_control())
        .collect()
}

/// Builds a raw OSC 52 clipboard-write sequence for the system clipboard.
///
/// This is the Rust terminal-output counterpart to CC Ink's `setClipboard` raw
/// sequence (`ESC ] 52 ; c ; <base64> BEL`). Multiplexer/native-clipboard
/// transports are owned by the shared Clipboard service; this helper also
/// preserves the legacy raw OSC path when no service is provided.
pub(crate) fn osc52_clipboard_sequence(text: &str) -> String {
    osc52_clipboard_sequence_with_kitty(text, false)
}

/// Clipboard callers supply the resolved terminal identity; tmux inner OSC
/// intentionally retains BEL regardless of the outer terminal.
pub(crate) fn osc52_clipboard_sequence_with_kitty(text: &str, is_kitty: bool) -> String {
    let terminator = if is_kitty { "\x1b\\" } else { "\x07" };
    format!("\x1b]52;c;{}{terminator}", base64_encode(text.as_bytes()))
}

pub(crate) fn osc52_clipboard_sequence_for_multiplexer(
    text: &str,
    multiplexer: MultiplexerPassthrough,
) -> String {
    wrap_for_multiplexer_sequence(&osc52_clipboard_sequence(text), multiplexer)
}

pub(crate) fn osc52_clipboard(w: &mut (impl Write + ?Sized), text: &str) -> io::Result<()> {
    w.write_all(osc52_clipboard_sequence(text).as_bytes())
}

pub(crate) fn osc52_clipboard_for_multiplexer(
    w: &mut (impl Write + ?Sized),
    text: &str,
    multiplexer: MultiplexerPassthrough,
) -> io::Result<()> {
    w.write_all(osc52_clipboard_sequence_for_multiplexer(text, multiplexer).as_bytes())
}

pub(crate) fn terminal_title(w: &mut (impl Write + ?Sized), title: &str) -> io::Result<()> {
    w.write_all(b"\x1b]0;")?;
    for ch in title.chars().filter(|ch| !ch.is_control()) {
        write!(w, "{ch}")?;
    }
    w.write_all(b"\x07")
}

#[cfg(test)]
mod tests {
    fn supports_hyperlinks_env<'a>(
        pairs: &'a [(&'a str, &'a str)],
        stdout_supported: bool,
    ) -> bool {
        super::supports_hyperlinks_with_env(
            |key| {
                pairs
                    .iter()
                    .find_map(|(k, v)| (*k == key).then(|| (*v).to_string()))
            },
            stdout_supported,
        )
    }

    #[test]
    fn supports_hyperlinks_matches_cc_extra_terminal_allowlist() {
        assert!(supports_hyperlinks_env(&[], true));
        assert!(supports_hyperlinks_env(
            &[("TERM_PROGRAM", "ghostty")],
            false
        ));
        assert!(supports_hyperlinks_env(&[("TERM_PROGRAM", "kitty")], false));
        assert!(supports_hyperlinks_env(&[("LC_TERMINAL", "iTerm2")], false));
        assert!(supports_hyperlinks_env(&[("TERM", "xterm-kitty")], false));
        assert!(!supports_hyperlinks_env(&[("TERM_PROGRAM", "dumb")], false));
    }

    fn sgr_bytes(color: super::Color, channel: super::ColorChannel, level: u8) -> String {
        let mut buf = Vec::new();
        super::sgr_color(&mut buf, color, channel, level).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn color_encoding_matches_chalk_model_selection() {
        use super::ColorChannel::{Background, Foreground};
        let orange = super::Color::Rgb {
            r: 215,
            g: 119,
            b: 87,
        };
        // Level 3 passes truecolor through (chalk wrapAnsi16m).
        assert_eq!(sgr_bytes(orange, Foreground, 3), "\x1b[38;2;215;119;87m");
        assert_eq!(sgr_bytes(orange, Background, 3), "\x1b[48;2;215;119;87m");
        // Level 2 downgrades via rgbToAnsi256; node oracle says 174.
        assert_eq!(sgr_bytes(orange, Foreground, 2), "\x1b[38;5;174m");
        assert_eq!(sgr_bytes(orange, Background, 2), "\x1b[48;5;174m");
        // Level 1 collapses onto the 16-color range; node oracle says 31/41.
        assert_eq!(sgr_bytes(orange, Foreground, 1), "\x1b[31m");
        assert_eq!(sgr_bytes(orange, Background, 1), "\x1b[41m");
    }

    #[test]
    fn named_colors_encode_as_chalk_bare_codes() {
        use super::ColorChannel::{Background, Foreground, Underline};
        // Bun + rebuild's ansi-styles oracle, including crossterm's bright
        // unprefixed names: Blue is blueBright (94 foreground, 104 background).
        let cases = [
            (super::Color::Black, 30, 40, 0),
            (super::Color::DarkRed, 31, 41, 1),
            (super::Color::DarkGreen, 32, 42, 2),
            (super::Color::DarkYellow, 33, 43, 3),
            (super::Color::DarkBlue, 34, 44, 4),
            (super::Color::DarkMagenta, 35, 45, 5),
            (super::Color::DarkCyan, 36, 46, 6),
            (super::Color::Grey, 37, 47, 7),
            (super::Color::DarkGrey, 90, 100, 8),
            (super::Color::Red, 91, 101, 9),
            (super::Color::Green, 92, 102, 10),
            (super::Color::Yellow, 93, 103, 11),
            (super::Color::Blue, 94, 104, 12),
            (super::Color::Magenta, 95, 105, 13),
            (super::Color::Cyan, 96, 106, 14),
            (super::Color::White, 97, 107, 15),
        ];
        for (color, foreground, background, palette) in cases {
            for level in 0..=3 {
                for (channel, expected) in [
                    (Foreground, format!("\x1b[{foreground}m")),
                    (Background, format!("\x1b[{background}m")),
                    // Underline uses SGR 58's palette form, not a basic code.
                    (Underline, format!("\x1b[58;5;{palette}m")),
                ] {
                    assert_eq!(
                        sgr_bytes(color, channel, level),
                        if level == 0 { "" } else { &expected },
                        "{color:?}, {channel:?}, level {level}"
                    );
                }
            }
        }
    }

    #[test]
    fn ansi256_values_collapse_only_below_level_two() {
        use super::ColorChannel::Foreground;
        assert_eq!(
            sgr_bytes(super::Color::AnsiValue(255), Foreground, 2),
            "\x1b[38;5;255m"
        );
        // node ansi-styles oracle: ansi256ToAnsi(255) → 37 (not bright).
        assert_eq!(
            sgr_bytes(super::Color::AnsiValue(255), Foreground, 1),
            "\x1b[37m"
        );
    }

    #[test]
    fn underline_colors_use_chalk_v6_palette_form() {
        use super::ColorChannel::Underline;
        let orange = super::Color::Rgb {
            r: 215,
            g: 119,
            b: 87,
        };
        // Named codes map onto palette indices (wrapUnderlineAnsi).
        assert_eq!(
            sgr_bytes(super::Color::DarkRed, Underline, 3),
            "\x1b[58;5;1m"
        );
        assert_eq!(sgr_bytes(super::Color::Red, Underline, 3), "\x1b[58;5;9m");
        assert_eq!(sgr_bytes(orange, Underline, 3), "\x1b[58;2;215;119;87m");
        assert_eq!(sgr_bytes(orange, Underline, 2), "\x1b[58;5;174m");
    }

    #[test]
    fn reset_uses_channel_close_codes_and_level_zero_suppresses_colors() {
        use super::ColorChannel::{Background, Foreground, Underline};
        for level in 0..=3 {
            for (channel, expected) in [
                (Foreground, "\x1b[39m"),
                (Background, "\x1b[49m"),
                (Underline, "\x1b[59m"),
            ] {
                assert_eq!(
                    sgr_bytes(super::Color::Reset, channel, level),
                    if level == 0 { "" } else { expected }
                );
            }
        }
        // The color half of level 0's full passthrough: this encoder emits
        // nothing, including close codes. The attribute half lives in the
        // transition writers' `styles_enabled` gate.
        let orange = super::Color::Rgb {
            r: 215,
            g: 119,
            b: 87,
        };
        assert_eq!(sgr_bytes(orange, Foreground, 0), "");
        assert_eq!(sgr_bytes(super::Color::Reset, Background, 0), "");
    }

    #[test]
    fn all_palette_entries_match_bun_downgrade_oracle() {
        use super::ColorChannel::{Background, Foreground, Underline};
        // Literal ansi256ToAnsi results from rebuild's ansi-styles under Bun.
        const ANSI16: [u8; 256] = [
            30, 31, 32, 33, 34, 35, 36, 37, 90, 91, 92, 93, 94, 95, 96, 97, 30, 30, 30, 34, 34, 94,
            30, 30, 30, 34, 34, 94, 30, 30, 30, 34, 34, 94, 32, 32, 32, 36, 36, 96, 32, 32, 32, 36,
            36, 96, 92, 92, 92, 96, 96, 96, 30, 30, 30, 34, 34, 94, 30, 30, 30, 34, 34, 94, 30, 30,
            30, 34, 34, 94, 32, 32, 32, 36, 36, 96, 32, 32, 32, 36, 36, 96, 92, 92, 92, 96, 96, 96,
            30, 30, 30, 34, 34, 94, 30, 30, 30, 34, 34, 94, 30, 30, 30, 34, 34, 94, 32, 32, 32, 36,
            36, 96, 32, 32, 32, 36, 36, 96, 92, 92, 92, 96, 96, 96, 31, 31, 31, 35, 35, 95, 31, 31,
            31, 35, 35, 95, 31, 31, 31, 35, 35, 95, 33, 33, 33, 37, 37, 97, 33, 33, 33, 37, 37, 97,
            93, 93, 93, 97, 97, 97, 31, 31, 31, 35, 35, 95, 31, 31, 31, 35, 35, 95, 31, 31, 31, 35,
            35, 95, 33, 33, 33, 37, 37, 97, 33, 33, 33, 37, 37, 97, 93, 93, 93, 97, 97, 97, 91, 91,
            91, 95, 95, 95, 91, 91, 91, 95, 95, 95, 91, 91, 91, 95, 95, 95, 93, 93, 93, 97, 97, 97,
            93, 93, 93, 97, 97, 97, 93, 93, 93, 97, 97, 97, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
            30, 30, 37, 37, 37, 37, 37, 37, 37, 37, 37, 37, 37, 37,
        ];
        for (index, basic) in ANSI16.into_iter().enumerate() {
            for level in 0..=3 {
                let color = super::Color::AnsiValue(index as u8);
                let palette = if level == 1 {
                    if basic < 90 {
                        basic - 30
                    } else {
                        basic - 90 + 8
                    }
                } else {
                    index as u8
                };
                for (channel, expected) in [
                    (
                        Foreground,
                        if level == 1 {
                            format!("\x1b[{basic}m")
                        } else {
                            format!("\x1b[38;5;{index}m")
                        },
                    ),
                    (
                        Background,
                        if level == 1 {
                            format!("\x1b[{}m", basic + 10)
                        } else {
                            format!("\x1b[48;5;{index}m")
                        },
                    ),
                    (Underline, format!("\x1b[58;5;{palette}m")),
                ] {
                    assert_eq!(
                        sgr_bytes(color, channel, level),
                        if level == 0 { "" } else { &expected },
                        "palette {index}, {channel:?}, level {level}"
                    );
                }
            }
        }
    }

    #[test]
    fn rgb_boundaries_match_bun_downgrade_oracle() {
        use super::ColorChannel::{Background, Foreground, Underline};
        // RGB + literal rgbToAnsi256/rgbToAnsi results from Bun's source
        // oracle: grayscale endpoints and color-cube rounding boundaries.
        let cases = [
            (0, 0, 0, 16, 30),
            (7, 7, 7, 16, 30),
            (8, 8, 8, 232, 30),
            (127, 127, 127, 244, 37),
            (128, 128, 128, 244, 37),
            (248, 248, 248, 255, 37),
            (249, 249, 249, 231, 97),
            (255, 255, 255, 231, 97),
            (255, 0, 0, 196, 91),
            (0, 255, 0, 46, 92),
            (0, 0, 255, 21, 94),
            (255, 255, 0, 226, 93),
            (0, 255, 255, 51, 96),
            (255, 0, 255, 201, 95),
            (215, 119, 87, 174, 31),
            (25, 76, 127, 24, 30),
            (26, 77, 128, 67, 34),
        ];
        for (r, g, b, palette, basic) in cases {
            let color = super::Color::Rgb { r, g, b };
            for level in 0..=3 {
                for (channel, prefix) in [(Foreground, 38), (Background, 48), (Underline, 58)] {
                    let expected = match level {
                        0 => String::new(),
                        3 => format!("\x1b[{prefix};2;{r};{g};{b}m"),
                        2 => format!("\x1b[{prefix};5;{palette}m"),
                        _ if channel == Underline => {
                            let index = if basic < 90 {
                                basic - 30
                            } else {
                                basic - 90 + 8
                            };
                            format!("\x1b[58;5;{index}m")
                        }
                        _ => format!(
                            "\x1b[{}m",
                            basic + if channel == Background { 10 } else { 0 }
                        ),
                    };
                    assert_eq!(
                        sgr_bytes(color, channel, level),
                        expected,
                        "{color:?}, {channel:?}, level {level}"
                    );
                }
            }
        }
    }

    #[test]
    fn osc8_id_matches_cc_ink_hash_and_sanitizes_href() {
        assert_eq!(super::osc8_id("https://example.com"), "ags5vy");

        let mut buf = Vec::new();
        super::hyperlink_open(&mut buf, "https://safe.example/\x1b]0;owned\x07").unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(
            output,
            "\x1b]8;id=6cevo;https://safe.example/]0;owned\x1b\\"
        );
        assert!(!output.contains("\x1b]0;owned"));
    }

    #[test]
    fn osc52_clipboard_sequence_base64_encodes_utf8_text() {
        assert_eq!(super::osc52_clipboard_sequence(""), "\x1b]52;c;\x07");
        assert_eq!(super::osc52_clipboard_sequence("f"), "\x1b]52;c;Zg==\x07");
        assert_eq!(super::osc52_clipboard_sequence("fo"), "\x1b]52;c;Zm8=\x07");
        assert_eq!(super::osc52_clipboard_sequence("foo"), "\x1b]52;c;Zm9v\x07");
        assert_eq!(
            super::osc52_clipboard_sequence("中文"),
            "\x1b]52;c;5Lit5paH\x07"
        );
    }

    #[test]
    fn multiplexer_passthrough_wraps_clipboard_sequences() {
        let raw = "\x1b]52;c;Y29weQ==\x07";
        assert_eq!(
            super::wrap_for_multiplexer_sequence(raw, super::MultiplexerPassthrough::Tmux),
            "\x1bPtmux;\x1b\x1b]52;c;Y29weQ==\x07\x1b\\"
        );
        assert_eq!(
            super::wrap_for_multiplexer_sequence(raw, super::MultiplexerPassthrough::Screen),
            "\x1bP\x1b]52;c;Y29weQ==\x07\x1b\\"
        );
        assert_eq!(
            super::osc52_clipboard_sequence_for_multiplexer(
                "copy",
                super::MultiplexerPassthrough::Tmux,
            ),
            "\x1bPtmux;\x1b\x1b]52;c;Y29weQ==\x07\x1b\\"
        );
    }

    #[test]
    fn terminal_title_filters_control_chars() {
        let mut buf = Vec::new();
        super::terminal_title(&mut buf, "safe\x1b]2;owned\x07").unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output, "\x1b]0;safe]2;owned\x07");
        assert!(!output.contains("\x1b]2;owned"));
    }
}

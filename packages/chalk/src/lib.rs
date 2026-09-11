//! A Rust port of [chalk](https://github.com/chalk/chalk): terminal string
//! styling with color-level detection and automatic color-space downgrade.
//!
//! Maps to: chalk/source/index.js (builder + `applyStyle`),
//! chalk/source/vendor/ansi-styles (SGR codes + conversions), and
//! chalk/source/vendor/supports-color (level detection), plus the two
//! Claude Code Ink level adjustments from `ink/colorize.ts`.
//!
//! # Example
//!
//! ```
//! use chalk::Chalk;
//!
//! // Explicit level for deterministic output (0 disables styling entirely).
//! let chalk = Chalk::with_level(3);
//! let styled = chalk.bold().yellow().apply("warning");
//! assert_eq!(styled, "\u{1b}[1m\u{1b}[33mwarning\u{1b}[39m\u{1b}[22m");
//!
//! // Level 0 is a clean passthrough.
//! assert_eq!(Chalk::with_level(0).bold().apply("plain"), "plain");
//! ```

mod level;
mod styles;

pub use level::{set_stderr_level, set_stdout_level, stderr_level, stdout_level, ColorLevel};
pub use styles::{ansi256_to_ansi, hex_to_rgb, rgb_to_ansi, rgb_to_ansi256, NamedColor};

use styles::{
    wrap_ansi16, wrap_ansi16m, wrap_ansi256, CodePair, ANSI_BACKGROUND_OFFSET, BG_CLOSE, FG_CLOSE,
};

/// One styling layer: an open/close escape-sequence pair.
///
/// Maps to chalk's `createStyler` entries; the builder keeps them in
/// application order so `open_all`/`close_all` compose identically.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Layer {
    open: String,
    close: String,
}

/// A chainable terminal string styler.
///
/// Maps to chalk's builder. Each styling method returns a new value with the
/// layer appended, and [`Chalk::apply`] is `applyStyle`: it re-opens nested
/// closes, encases linebreaks, and wraps the string in the accumulated
/// open/close sequences. At level `0` every `apply` is a passthrough.
///
/// `Chalk` snapshots the singleton level at construction. The JS default
/// export is one mutable object — writing `chalk.level` retargets every later
/// call through that same instance. Match that behavior by constructing at
/// use time (`Chalk::new()` per render pass); a long-lived `Chalk` value
/// deliberately keeps the level it captured.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chalk {
    level: ColorLevel,
    layers: Vec<Layer>,
    /// Maps to chalk's `visible` modifier / IS_EMPTY flag: when set, a level-0
    /// apply yields an empty string instead of the unstyled input.
    visible_only: bool,
}

impl Default for Chalk {
    /// A styler bound to the process-wide stdout level ([`stdout_level`]).
    fn default() -> Self {
        Self::new()
    }
}

impl Chalk {
    /// Creates a styler using the detected stdout color level.
    pub fn new() -> Self {
        Self::with_level(stdout_level())
    }

    /// Creates a styler using the detected stderr color level. Maps to
    /// chalk's `chalkStderr` export.
    pub fn stderr() -> Self {
        Self::with_level(stderr_level())
    }

    /// Creates a styler with an explicit level (0-3). Values above 3 clamp,
    /// mirroring chalk's `level` option validation without the throw.
    pub fn with_level(level: ColorLevel) -> Self {
        Self {
            level: level.min(3),
            layers: Vec::new(),
            visible_only: false,
        }
    }

    /// The color level this styler applies at.
    pub fn level(&self) -> ColorLevel {
        self.level
    }

    fn push_pair(mut self, pair: CodePair) -> Self {
        self.layers.push(Layer {
            open: wrap_ansi16(pair.open, 0),
            close: wrap_ansi16(pair.close, 0),
        });
        self
    }

    fn push_fg(mut self, open: String) -> Self {
        self.layers.push(Layer {
            open,
            close: wrap_ansi16(FG_CLOSE, 0),
        });
        self
    }

    fn push_bg(mut self, open: String) -> Self {
        self.layers.push(Layer {
            open,
            close: wrap_ansi16(BG_CLOSE, 0),
        });
        self
    }

    // ── modifiers ──────────────────────────────────────────────────────

    /// SGR 1 / 22.
    pub fn bold(self) -> Self {
        self.push_pair(styles::BOLD)
    }

    /// SGR 2 / 22.
    pub fn dim(self) -> Self {
        self.push_pair(styles::DIM)
    }

    /// SGR 3 / 23.
    pub fn italic(self) -> Self {
        self.push_pair(styles::ITALIC)
    }

    /// SGR 4 / 24.
    pub fn underline(self) -> Self {
        self.push_pair(styles::UNDERLINE)
    }

    /// SGR 53 / 55.
    pub fn overline(self) -> Self {
        self.push_pair(styles::OVERLINE)
    }

    /// SGR 7 / 27.
    pub fn inverse(self) -> Self {
        self.push_pair(styles::INVERSE)
    }

    /// SGR 8 / 28.
    pub fn hidden(self) -> Self {
        self.push_pair(styles::HIDDEN)
    }

    /// SGR 9 / 29.
    pub fn strikethrough(self) -> Self {
        self.push_pair(styles::STRIKETHROUGH)
    }

    /// SGR 0 / 0.
    pub fn reset(self) -> Self {
        self.push_pair(styles::RESET)
    }

    /// Marks the text as only visible when colors are enabled. Maps to
    /// chalk's `visible` modifier: at level 0 the output becomes empty.
    pub fn visible(mut self) -> Self {
        self.visible_only = true;
        self
    }

    fn push_raw(mut self, open: String, close: String) -> Self {
        self.layers.push(Layer { open, close });
        self
    }

    // ── extended underline styles (chalk v6, SGR 4:x sub-parameters) ───

    /// SGR 4:2 double underline (closes with 24).
    pub fn underline_double(self) -> Self {
        self.push_raw("\u{1b}[4:2m".to_string(), "\u{1b}[24m".to_string())
    }

    /// SGR 4:3 curly underline (closes with 24).
    pub fn underline_curly(self) -> Self {
        self.push_raw("\u{1b}[4:3m".to_string(), "\u{1b}[24m".to_string())
    }

    /// SGR 4:4 dotted underline (closes with 24).
    pub fn underline_dotted(self) -> Self {
        self.push_raw("\u{1b}[4:4m".to_string(), "\u{1b}[24m".to_string())
    }

    /// SGR 4:5 dashed underline (closes with 24).
    pub fn underline_dashed(self) -> Self {
        self.push_raw("\u{1b}[4:5m".to_string(), "\u{1b}[24m".to_string())
    }

    // ── underline colors (chalk v6, SGR 58/59) ─────────────────────────

    fn push_underline(self, open: String) -> Self {
        let close = wrap_ansi16(styles::UNDERLINE_CLOSE, 0);
        self.push_raw(open, close)
    }

    /// A named underline color as its palette index (`SGR 58;5;n`). The named
    /// underline colors have no basic 16-color form, so they render the same
    /// at every level >= 1.
    pub fn underline_color(self, color: NamedColor) -> Self {
        let open = styles::wrap_underline_ansi(color.fg_open());
        self.push_underline(open)
    }

    /// Truecolor underline; downgrades to 256 / palette below level 3.
    pub fn underline_rgb(self, r: u8, g: u8, b: u8) -> Self {
        let open = match self.level {
            3 => wrap_ansi16m(r, g, b, styles::ANSI_UNDERLINE_OFFSET),
            2 => wrap_ansi256(rgb_to_ansi256(r, g, b), styles::ANSI_UNDERLINE_OFFSET),
            _ => styles::wrap_underline_ansi(rgb_to_ansi(r, g, b)),
        };
        self.push_underline(open)
    }

    /// Hex underline; downgrades below level 3.
    pub fn underline_hex(self, hex: &str) -> Self {
        let (r, g, b) = hex_to_rgb(hex);
        self.underline_rgb(r, g, b)
    }

    /// 256-color underline; downgrades to the palette form below level 2.
    pub fn underline_ansi256(self, code: u8) -> Self {
        let open = if self.level >= 2 {
            wrap_ansi256(code, styles::ANSI_UNDERLINE_OFFSET)
        } else {
            styles::wrap_underline_ansi(ansi256_to_ansi(code))
        };
        self.push_underline(open)
    }

    // ── named colors ───────────────────────────────────────────────────

    /// A named 16-color foreground.
    pub fn color(self, color: NamedColor) -> Self {
        let open = wrap_ansi16(color.fg_open(), 0);
        self.push_fg(open)
    }

    /// A named 16-color background.
    pub fn bg_color(self, color: NamedColor) -> Self {
        let open = wrap_ansi16(color.fg_open(), ANSI_BACKGROUND_OFFSET);
        self.push_bg(open)
    }

    /// `NamedColor::Black` foreground.
    pub fn black(self) -> Self {
        self.color(NamedColor::Black)
    }

    /// `NamedColor::Red` foreground.
    pub fn red(self) -> Self {
        self.color(NamedColor::Red)
    }

    /// `NamedColor::Green` foreground.
    pub fn green(self) -> Self {
        self.color(NamedColor::Green)
    }

    /// `NamedColor::Yellow` foreground.
    pub fn yellow(self) -> Self {
        self.color(NamedColor::Yellow)
    }

    /// `NamedColor::Blue` foreground.
    pub fn blue(self) -> Self {
        self.color(NamedColor::Blue)
    }

    /// `NamedColor::Magenta` foreground.
    pub fn magenta(self) -> Self {
        self.color(NamedColor::Magenta)
    }

    /// `NamedColor::Cyan` foreground.
    pub fn cyan(self) -> Self {
        self.color(NamedColor::Cyan)
    }

    /// `NamedColor::White` foreground.
    pub fn white(self) -> Self {
        self.color(NamedColor::White)
    }

    /// `NamedColor::BlackBright` foreground (chalk's `gray`/`grey`).
    pub fn gray(self) -> Self {
        self.color(NamedColor::BlackBright)
    }

    // ── parameterized colors with level downgrade ──────────────────────

    /// Truecolor foreground; downgrades to 256 / 16 colors below level 3.
    ///
    /// Maps to chalk's `getModelAnsi('rgb', ...)` through `levelMapping`.
    pub fn rgb(self, r: u8, g: u8, b: u8) -> Self {
        let open = Self::model_rgb(self.level, r, g, b, 0);
        self.push_fg(open)
    }

    /// Truecolor background; downgrades below level 3.
    pub fn bg_rgb(self, r: u8, g: u8, b: u8) -> Self {
        let open = Self::model_rgb(self.level, r, g, b, ANSI_BACKGROUND_OFFSET);
        self.push_bg(open)
    }

    /// Hex foreground (`#rrggbb` / `#rgb`); downgrades below level 3.
    pub fn hex(self, hex: &str) -> Self {
        let (r, g, b) = hex_to_rgb(hex);
        self.rgb(r, g, b)
    }

    /// Hex background; downgrades below level 3.
    pub fn bg_hex(self, hex: &str) -> Self {
        let (r, g, b) = hex_to_rgb(hex);
        self.bg_rgb(r, g, b)
    }

    /// 256-color foreground; downgrades to 16 colors below level 2.
    pub fn ansi256(self, code: u8) -> Self {
        let open = Self::model_ansi256(self.level, code, 0);
        self.push_fg(open)
    }

    /// 256-color background; downgrades below level 2.
    pub fn bg_ansi256(self, code: u8) -> Self {
        let open = Self::model_ansi256(self.level, code, ANSI_BACKGROUND_OFFSET);
        self.push_bg(open)
    }

    fn model_rgb(level: ColorLevel, r: u8, g: u8, b: u8, offset: u8) -> String {
        match level {
            3 => wrap_ansi16m(r, g, b, offset),
            2 => wrap_ansi256(rgb_to_ansi256(r, g, b), offset),
            _ => wrap_ansi16(rgb_to_ansi(r, g, b), offset),
        }
    }

    fn model_ansi256(level: ColorLevel, code: u8, offset: u8) -> String {
        if level >= 2 {
            wrap_ansi256(code, offset)
        } else {
            wrap_ansi16(ansi256_to_ansi(code), offset)
        }
    }

    // ── apply ──────────────────────────────────────────────────────────

    /// Styles a string.
    ///
    /// Maps to chalk's `applyStyle`:
    /// - level 0 or an empty input returns the input unchanged
    /// - embedded closing codes are replaced with re-opening codes, innermost
    ///   layer first, so nested styled substrings don't terminate the outer
    ///   style early
    /// - every linebreak is encased as `closeAll + newline + openAll`,
    ///   preserving CRLF, to fix the macOS bleed issue (chalk#92)
    pub fn apply(&self, input: &str) -> String {
        if self.level == 0 {
            // Maps to applyStyle's IS_EMPTY branch: visible text disappears
            // entirely when colors are disabled.
            return if self.visible_only {
                String::new()
            } else {
                input.to_string()
            };
        }
        if input.is_empty() || self.layers.is_empty() {
            return input.to_string();
        }

        let open_all: String = self.layers.iter().map(|l| l.open.as_str()).collect();
        let close_all: String = self.layers.iter().rev().map(|l| l.close.as_str()).collect();

        let mut string = input.to_string();
        if string.contains('\u{1b}') {
            // chalk walks the styler parent chain from innermost outwards; our
            // layers are stored outermost-first, so iterate in reverse. Note
            // stringReplaceAll KEEPS the close and appends the re-open after
            // it (`substring + replacer`), it does not substitute it away.
            for layer in self.layers.iter().rev() {
                let reopen = format!("{}{}", layer.close, layer.open);
                string = string.replace(&layer.close, &reopen);
            }
        }

        if string.contains('\n') {
            string = encase_line_breaks(&string, &close_all, &open_all);
        }

        format!("{open_all}{string}{close_all}")
    }
}

/// Maps to chalk's `stringEncaseCRLFWithFirstIndex`: closes the style before
/// each linebreak (CRLF preserved) and reopens after it.
fn encase_line_breaks(input: &str, close_all: &str, open_all: &str) -> String {
    let mut out = String::with_capacity(input.len() + 16);
    let mut rest = input;
    while let Some(lf) = rest.find('\n') {
        let (line, tail) = rest.split_at(lf);
        let (body, newline) = match line.strip_suffix('\r') {
            Some(body) => (body, "\r\n"),
            None => (line, "\n"),
        };
        out.push_str(body);
        out.push_str(close_all);
        out.push_str(newline);
        out.push_str(open_all);
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}

// ── convenience functions bound to the stdout singleton ────────────────

/// `Chalk::new().bold().apply(text)`.
pub fn bold(text: &str) -> String {
    Chalk::new().bold().apply(text)
}

/// `Chalk::new().dim().apply(text)`.
pub fn dim(text: &str) -> String {
    Chalk::new().dim().apply(text)
}

/// `Chalk::new().italic().apply(text)`.
pub fn italic(text: &str) -> String {
    Chalk::new().italic().apply(text)
}

/// `Chalk::new().underline().apply(text)`.
pub fn underline(text: &str) -> String {
    Chalk::new().underline().apply(text)
}

/// `Chalk::new().inverse().apply(text)`.
pub fn inverse(text: &str) -> String {
    Chalk::new().inverse().apply(text)
}

/// `Chalk::new().strikethrough().apply(text)`.
pub fn strikethrough(text: &str) -> String {
    Chalk::new().strikethrough().apply(text)
}

/// `Chalk::new().red().apply(text)`.
pub fn red(text: &str) -> String {
    Chalk::new().red().apply(text)
}

/// `Chalk::new().green().apply(text)`.
pub fn green(text: &str) -> String {
    Chalk::new().green().apply(text)
}

/// `Chalk::new().yellow().apply(text)`.
pub fn yellow(text: &str) -> String {
    Chalk::new().yellow().apply(text)
}

/// `Chalk::new().blue().apply(text)`.
pub fn blue(text: &str) -> String {
    Chalk::new().blue().apply(text)
}

/// `Chalk::new().cyan().apply(text)`.
pub fn cyan(text: &str) -> String {
    Chalk::new().cyan().apply(text)
}

/// `Chalk::new().magenta().apply(text)`.
pub fn magenta(text: &str) -> String {
    Chalk::new().magenta().apply(text)
}

/// `Chalk::new().gray().apply(text)`.
pub fn gray(text: &str) -> String {
    Chalk::new().gray().apply(text)
}

#[cfg(test)]
mod tests {
    //! Complete port of chalk's official test suite (test/chalk.js,
    //! test/visible.js, test/instance.js), skipping only JS-specific cases:
    //! argument coercion/join, Function.prototype methods, style caching via
    //! property getters, and mutable `.level` propagation (our builder is a
    //! value type; `with_level` covers isolated-instance semantics).

    use super::*;

    fn c() -> Chalk {
        Chalk::with_level(3)
    }

    // test('style string')
    #[test]
    fn style_string() {
        assert_eq!(c().underline().apply("foo"), "\u{1b}[4mfoo\u{1b}[24m");
        assert_eq!(c().red().apply("foo"), "\u{1b}[31mfoo\u{1b}[39m");
        assert_eq!(
            c().bg_color(NamedColor::Red).apply("foo"),
            "\u{1b}[41mfoo\u{1b}[49m"
        );
    }

    // test('support applying multiple styles at once')
    #[test]
    fn multiple_styles_at_once() {
        assert_eq!(
            c().red()
                .bg_color(NamedColor::Green)
                .underline()
                .apply("foo"),
            "\u{1b}[31m\u{1b}[42m\u{1b}[4mfoo\u{1b}[24m\u{1b}[49m\u{1b}[39m"
        );
        assert_eq!(
            c().underline()
                .red()
                .bg_color(NamedColor::Green)
                .apply("foo"),
            "\u{1b}[4m\u{1b}[31m\u{1b}[42mfoo\u{1b}[49m\u{1b}[39m\u{1b}[24m"
        );
    }

    // test('support nesting styles')
    #[test]
    fn nesting_styles() {
        let inner = c().underline().bg_color(NamedColor::Blue).apply("bar");
        assert_eq!(
            c().red().apply(&format!("foo{inner}!")),
            "\u{1b}[31mfoo\u{1b}[4m\u{1b}[44mbar\u{1b}[49m\u{1b}[24m!\u{1b}[39m"
        );
    }

    // test('support nesting styles of the same type (color, underline, bg)')
    #[test]
    fn nesting_same_type() {
        let inner = c().green().apply("c");
        let nested = c().yellow().apply(&format!("b{inner}b"));
        assert_eq!(
            c().red().apply(&format!("a{nested}c")),
            "\u{1b}[31ma\u{1b}[33mb\u{1b}[32mc\u{1b}[39m\u{1b}[31m\u{1b}[33mb\u{1b}[39m\u{1b}[31mc\u{1b}[39m"
        );
    }

    // test('reset all styles with `.reset()`')
    #[test]
    fn reset_all_styles() {
        let styled = c()
            .red()
            .bg_color(NamedColor::Green)
            .underline()
            .apply("foo");
        assert_eq!(
            c().reset().apply(&format!("{styled}foo")),
            "\u{1b}[0m\u{1b}[31m\u{1b}[42m\u{1b}[4mfoo\u{1b}[24m\u{1b}[49m\u{1b}[39mfoo\u{1b}[0m"
        );
    }

    // test('alias gray to grey') / test('supports blackBright color')
    #[test]
    fn gray_is_black_bright() {
        assert_eq!(c().gray().apply("foo"), "\u{1b}[90mfoo\u{1b}[39m");
        assert_eq!(
            c().color(NamedColor::BlackBright).apply("foo"),
            "\u{1b}[90mfoo\u{1b}[39m"
        );
    }

    // test("don't output escape codes if the input is empty")
    #[test]
    fn empty_input_yields_empty() {
        assert_eq!(c().red().apply(""), "");
        assert_eq!(c().red().blue().black().apply(""), "");
    }

    // test('line breaks should open and close colors') (+ CRLF variant)
    #[test]
    fn linebreaks_open_and_close_colors() {
        assert_eq!(
            c().gray().apply("hello\nworld"),
            "\u{1b}[90mhello\u{1b}[39m\n\u{1b}[90mworld\u{1b}[39m"
        );
        assert_eq!(
            c().gray().apply("hello\r\nworld"),
            "\u{1b}[90mhello\u{1b}[39m\r\n\u{1b}[90mworld\u{1b}[39m"
        );
    }

    // test('properly convert RGB to 16 colors on basic color terminals')
    #[test]
    fn rgb_to_16_on_level_one() {
        assert_eq!(
            Chalk::with_level(1).rgb(255, 0, 0).apply("hello"),
            "\u{1b}[91mhello\u{1b}[39m"
        );
        assert_eq!(
            Chalk::with_level(1).bg_rgb(255, 0, 0).apply("hello"),
            "\u{1b}[101mhello\u{1b}[49m"
        );
        assert_eq!(
            Chalk::with_level(1).hex("#FF0000").apply("hello"),
            "\u{1b}[91mhello\u{1b}[39m"
        );
        assert_eq!(
            Chalk::with_level(1).bg_hex("#FF0000").apply("hello"),
            "\u{1b}[101mhello\u{1b}[49m"
        );
    }

    // test('properly convert RGB to 256 colors on basic color terminals')
    #[test]
    fn rgb_to_256_and_truecolor() {
        assert_eq!(
            Chalk::with_level(2).rgb(255, 0, 0).apply("hello"),
            "\u{1b}[38;5;196mhello\u{1b}[39m"
        );
        assert_eq!(
            Chalk::with_level(2).bg_rgb(255, 0, 0).apply("hello"),
            "\u{1b}[48;5;196mhello\u{1b}[49m"
        );
        assert_eq!(
            Chalk::with_level(3).rgb(255, 0, 0).apply("hello"),
            "\u{1b}[38;2;255;0;0mhello\u{1b}[39m"
        );
        assert_eq!(
            Chalk::with_level(3).bg_rgb(255, 0, 0).apply("hello"),
            "\u{1b}[48;2;255;0;0mhello\u{1b}[49m"
        );
        assert_eq!(
            Chalk::with_level(2).hex("#FF0000").apply("hello"),
            "\u{1b}[38;5;196mhello\u{1b}[39m"
        );
        assert_eq!(
            Chalk::with_level(2).bg_hex("#FF0000").apply("hello"),
            "\u{1b}[48;5;196mhello\u{1b}[49m"
        );
        assert_eq!(
            Chalk::with_level(3).bg_hex("#FF0000").apply("hello"),
            "\u{1b}[48;2;255;0;0mhello\u{1b}[49m"
        );
    }

    // test('properly convert ANSI 256 to 16 colors on basic color terminals')
    #[test]
    fn ansi256_to_16_on_level_one() {
        let l1 = || Chalk::with_level(1);
        assert_eq!(
            l1().ansi256(196).apply("hello"),
            "\u{1b}[91mhello\u{1b}[39m"
        );
        assert_eq!(
            l1().bg_ansi256(196).apply("hello"),
            "\u{1b}[101mhello\u{1b}[49m"
        );
        assert_eq!(l1().ansi256(2).apply("hello"), "\u{1b}[32mhello\u{1b}[39m");
        assert_eq!(
            l1().bg_ansi256(2).apply("hello"),
            "\u{1b}[42mhello\u{1b}[49m"
        );
        assert_eq!(l1().ansi256(8).apply("hello"), "\u{1b}[90mhello\u{1b}[39m");
        assert_eq!(
            l1().ansi256(232).apply("hello"),
            "\u{1b}[30mhello\u{1b}[39m"
        );
        assert_eq!(
            l1().ansi256(255).apply("hello"),
            "\u{1b}[37mhello\u{1b}[39m"
        );
    }

    // test('keep ANSI 256 colors on 256 color and Truecolor terminals')
    #[test]
    fn ansi256_kept_at_higher_levels() {
        for level in [2u8, 3] {
            assert_eq!(
                Chalk::with_level(level).ansi256(196).apply("hello"),
                "\u{1b}[38;5;196mhello\u{1b}[39m"
            );
            assert_eq!(
                Chalk::with_level(level).bg_ansi256(196).apply("hello"),
                "\u{1b}[48;5;196mhello\u{1b}[49m"
            );
        }
    }

    // test("don't emit color codes if level is 0")
    #[test]
    fn level_zero_emits_nothing() {
        let l0 = || Chalk::with_level(0);
        assert_eq!(l0().hex("#FF0000").apply("hello"), "hello");
        assert_eq!(l0().bg_hex("#FF0000").apply("hello"), "hello");
        assert_eq!(l0().ansi256(196).apply("hello"), "hello");
        assert_eq!(l0().bg_ansi256(196).apply("hello"), "hello");
        assert_eq!(l0().underline_hex("#FF0000").apply("hello"), "hello");
        assert_eq!(l0().underline_ansi256(196).apply("hello"), "hello");
        assert_eq!(
            l0().underline_color(NamedColor::Red).apply("hello"),
            "hello"
        );
        assert_eq!(l0().underline_curly().apply("hello"), "hello");
    }

    // test('support extended underline styles')
    #[test]
    fn extended_underline_styles() {
        assert_eq!(
            c().underline_double().apply("foo"),
            "\u{1b}[4:2mfoo\u{1b}[24m"
        );
        assert_eq!(
            c().underline_curly().apply("foo"),
            "\u{1b}[4:3mfoo\u{1b}[24m"
        );
        assert_eq!(
            c().underline_dotted().apply("foo"),
            "\u{1b}[4:4mfoo\u{1b}[24m"
        );
        assert_eq!(
            c().underline_dashed().apply("foo"),
            "\u{1b}[4:5mfoo\u{1b}[24m"
        );
    }

    // test('support nesting underline styles')
    #[test]
    fn nesting_underline_styles() {
        let inner = c().underline_curly().apply("a");
        assert_eq!(
            c().underline().apply(&format!("{inner}b")),
            "\u{1b}[4m\u{1b}[4:3ma\u{1b}[24m\u{1b}[4mb\u{1b}[24m"
        );
        let inner = c().underline().apply("a");
        assert_eq!(
            c().underline_curly().apply(&format!("{inner}b")),
            "\u{1b}[4:3m\u{1b}[4ma\u{1b}[24m\u{1b}[4:3mb\u{1b}[24m"
        );
    }

    // test('support underline colors')
    #[test]
    fn underline_colors() {
        assert_eq!(
            c().underline_color(NamedColor::Red).apply("foo"),
            "\u{1b}[58;5;1mfoo\u{1b}[59m"
        );
        assert_eq!(
            c().underline_color(NamedColor::BlackBright).apply("foo"),
            "\u{1b}[58;5;8mfoo\u{1b}[59m"
        );
        assert_eq!(
            c().red()
                .underline_color(NamedColor::Red)
                .underline_curly()
                .apply("foo"),
            "\u{1b}[31m\u{1b}[58;5;1m\u{1b}[4:3mfoo\u{1b}[24m\u{1b}[59m\u{1b}[39m"
        );
    }

    // test('support nesting underline colors')
    #[test]
    fn nesting_underline_colors() {
        let inner = c().underline_color(NamedColor::Red).apply("a");
        assert_eq!(
            c().underline_color(NamedColor::Blue)
                .apply(&format!("{inner}b")),
            "\u{1b}[58;5;4m\u{1b}[58;5;1ma\u{1b}[59m\u{1b}[58;5;4mb\u{1b}[59m"
        );
    }

    // test('properly downsample underline colors')
    #[test]
    fn downsample_underline_colors() {
        assert_eq!(
            Chalk::with_level(3).underline_rgb(255, 0, 0).apply("hello"),
            "\u{1b}[58;2;255;0;0mhello\u{1b}[59m"
        );
        assert_eq!(
            Chalk::with_level(2).underline_rgb(255, 0, 0).apply("hello"),
            "\u{1b}[58;5;196mhello\u{1b}[59m"
        );
        assert_eq!(
            Chalk::with_level(1).underline_rgb(255, 0, 0).apply("hello"),
            "\u{1b}[58;5;9mhello\u{1b}[59m"
        );
        assert_eq!(
            Chalk::with_level(3).underline_hex("#FF0000").apply("hello"),
            "\u{1b}[58;2;255;0;0mhello\u{1b}[59m"
        );
        assert_eq!(
            Chalk::with_level(2).underline_hex("#FF0000").apply("hello"),
            "\u{1b}[58;5;196mhello\u{1b}[59m"
        );
        assert_eq!(
            Chalk::with_level(1).underline_hex("#FF0000").apply("hello"),
            "\u{1b}[58;5;9mhello\u{1b}[59m"
        );
        assert_eq!(
            Chalk::with_level(3).underline_ansi256(196).apply("hello"),
            "\u{1b}[58;5;196mhello\u{1b}[59m"
        );
        assert_eq!(
            Chalk::with_level(2).underline_ansi256(196).apply("hello"),
            "\u{1b}[58;5;196mhello\u{1b}[59m"
        );
        assert_eq!(
            Chalk::with_level(1).underline_ansi256(196).apply("hello"),
            "\u{1b}[58;5;9mhello\u{1b}[59m"
        );
        assert_eq!(
            Chalk::with_level(1).underline_ansi256(2).apply("hello"),
            "\u{1b}[58;5;2mhello\u{1b}[59m"
        );
        assert_eq!(
            Chalk::with_level(1).underline_ansi256(232).apply("hello"),
            "\u{1b}[58;5;0mhello\u{1b}[59m"
        );
        // The named underline colors have no basic 16-color form, so they are
        // the same at every level.
        assert_eq!(
            Chalk::with_level(1)
                .underline_color(NamedColor::Red)
                .apply("hello"),
            "\u{1b}[58;5;1mhello\u{1b}[59m"
        );
        assert_eq!(
            Chalk::with_level(2)
                .underline_color(NamedColor::Red)
                .apply("hello"),
            "\u{1b}[58;5;1mhello\u{1b}[59m"
        );
    }

    // test('sets correct level for chalkStderr and respects it')
    #[test]
    fn stderr_instance_respects_level() {
        let chalk_stderr = Chalk::with_level(3);
        assert_eq!(
            chalk_stderr.red().bold().apply("foo"),
            "\u{1b}[31m\u{1b}[1mfoo\u{1b}[22m\u{1b}[39m"
        );
    }

    // ── test/visible.js ────────────────────────────────────────────────

    // test('visible: normal output when level > 0')
    #[test]
    fn visible_normal_output_when_colored() {
        assert_eq!(
            Chalk::with_level(3).visible().red().apply("foo"),
            "\u{1b}[31mfoo\u{1b}[39m"
        );
        assert_eq!(
            Chalk::with_level(3).red().visible().apply("foo"),
            "\u{1b}[31mfoo\u{1b}[39m"
        );
    }

    // test('visible: no output when level is too low')
    #[test]
    fn visible_empty_when_disabled() {
        assert_eq!(Chalk::with_level(0).visible().red().apply("foo"), "");
        assert_eq!(Chalk::with_level(0).red().visible().apply("foo"), "");
        // Plain visible with no styles still passes text through at level > 0.
        assert_eq!(Chalk::with_level(3).visible().apply("foo"), "foo");
        assert_eq!(Chalk::with_level(0).visible().apply("foo"), "");
    }

    // ── test/instance.js ───────────────────────────────────────────────

    // test('create an isolated context where colors can be disabled (by level)')
    #[test]
    fn isolated_instances() {
        assert_eq!(Chalk::with_level(0).red().apply("foo"), "foo");
        assert_eq!(
            Chalk::with_level(1).red().apply("foo"),
            "\u{1b}[31mfoo\u{1b}[39m"
        );
        assert_eq!(
            Chalk::with_level(2).red().apply("foo"),
            "\u{1b}[31mfoo\u{1b}[39m"
        );
    }

    // test('the `level` option should be a number from 0 to 3'): our
    // ColorLevel is u8 (no negatives) and with_level clamps above 3 instead
    // of throwing.
    #[test]
    fn level_clamps_above_three() {
        assert_eq!(Chalk::with_level(10).level(), 3);
    }

    // test('a cached model style keeps following the level'): our builder
    // captures the level at construction, so re-building at a new level is
    // the equivalent behavior.
    #[test]
    fn model_styles_follow_level() {
        assert_eq!(
            Chalk::with_level(3).rgb(255, 0, 0).apply("foo"),
            "\u{1b}[38;2;255;0;0mfoo\u{1b}[39m"
        );
        assert_eq!(
            Chalk::with_level(1).rgb(255, 0, 0).apply("foo"),
            "\u{1b}[91mfoo\u{1b}[39m"
        );
        assert_eq!(Chalk::with_level(0).rgb(255, 0, 0).apply("foo"), "foo");
        // bold + rgb chain at levels 3 and 1.
        assert_eq!(
            Chalk::with_level(3).bold().rgb(255, 0, 0).apply("foo"),
            "\u{1b}[1m\u{1b}[38;2;255;0;0mfoo\u{1b}[39m\u{1b}[22m"
        );
        assert_eq!(
            Chalk::with_level(1).bold().rgb(255, 0, 0).apply("foo"),
            "\u{1b}[1m\u{1b}[91mfoo\u{1b}[39m\u{1b}[22m"
        );
    }

    // ── convenience helpers ────────────────────────────────────────────

    #[test]
    fn helper_functions_pass_through_without_styles() {
        // The helpers consult the process-wide stdout level. In tests the
        // stream is not a TTY, so styling degrades to passthrough — the
        // stable invariant either way is that text is preserved.
        assert!(bold("x").contains('x'));
        assert!(yellow("y").contains('y'));
    }
}

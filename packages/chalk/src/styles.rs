//! ANSI style codes and color-space conversions.
//!
//! Maps to: chalk/source/vendor/ansi-styles/index.js. Open/close code pairs,
//! the 16/256/16m wrappers, and the color-convert downgrade math.

/// A modifier or named-color style as an SGR open/close code pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CodePair {
    pub open: u8,
    pub close: u8,
}

macro_rules! code_pairs {
    ($($name:ident => ($open:expr, $close:expr)),* $(,)?) => {
        $(pub(crate) const $name: CodePair = CodePair { open: $open, close: $close };)*
    };
}

// modifier: [open, close]. 21 isn't widely supported and 22 does the same
// thing, so bold closes with 22 exactly like chalk.
code_pairs! {
    RESET => (0, 0),
    BOLD => (1, 22),
    DIM => (2, 22),
    ITALIC => (3, 23),
    UNDERLINE => (4, 24),
    OVERLINE => (53, 55),
    INVERSE => (7, 27),
    HIDDEN => (8, 28),
    STRIKETHROUGH => (9, 29),
}

/// Foreground close code shared by all colors.
pub(crate) const FG_CLOSE: u8 = 39;
/// Background close code shared by all colors.
pub(crate) const BG_CLOSE: u8 = 49;
/// Underline-color close code (SGR 59).
pub(crate) const UNDERLINE_CLOSE: u8 = 59;
/// SGR offset that turns a foreground code into its background variant.
pub(crate) const ANSI_BACKGROUND_OFFSET: u8 = 10;
/// SGR offset that turns a foreground code into its underline-color variant.
pub(crate) const ANSI_UNDERLINE_OFFSET: u8 = 20;

/// The 16 named colors as foreground open codes (bright variants included).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum NamedColor {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    BlackBright,
    RedBright,
    GreenBright,
    YellowBright,
    BlueBright,
    MagentaBright,
    CyanBright,
    WhiteBright,
}

impl NamedColor {
    pub(crate) fn fg_open(self) -> u8 {
        match self {
            Self::Black => 30,
            Self::Red => 31,
            Self::Green => 32,
            Self::Yellow => 33,
            Self::Blue => 34,
            Self::Magenta => 35,
            Self::Cyan => 36,
            Self::White => 37,
            Self::BlackBright => 90,
            Self::RedBright => 91,
            Self::GreenBright => 92,
            Self::YellowBright => 93,
            Self::BlueBright => 94,
            Self::MagentaBright => 95,
            Self::CyanBright => 96,
            Self::WhiteBright => 97,
        }
    }
}

/// Maps to ansi-styles `wrapAnsi16`.
pub(crate) fn wrap_ansi16(code: u8, offset: u8) -> String {
    format!("\u{1b}[{}m", code + offset)
}

/// Maps to ansi-styles `wrapAnsi256`.
pub(crate) fn wrap_ansi256(code: u8, offset: u8) -> String {
    format!("\u{1b}[{};5;{}m", 38 + offset, code)
}

/// Maps to ansi-styles `wrapAnsi16m`.
pub(crate) fn wrap_ansi16m(r: u8, g: u8, b: u8, offset: u8) -> String {
    format!("\u{1b}[{};2;{};{};{}m", 38 + offset, r, g, b)
}

/// Maps to chalk v6 ansi-styles `wrapUnderlineAnsi`: SGR 58 has no basic
/// 16-color form, so a 16-color SGR code maps to its palette index instead.
pub(crate) fn wrap_underline_ansi(code: u8) -> String {
    let palette = if code < 90 { code - 30 } else { code - 90 + 8 };
    format!("\u{1b}[58;5;{palette}m")
}

/// Maps to ansi-styles `rgbToAnsi256` (from color-convert): greyscale ramp for
/// equal channels, otherwise the 6x6x6 cube.
pub fn rgb_to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    if r == g && g == b {
        if r < 8 {
            return 16;
        }
        if r > 248 {
            return 231;
        }
        return js_round((f64::from(r) - 8.0) / 247.0 * 24.0) + 232;
    }
    16 + 36 * js_round(f64::from(r) / 255.0 * 5.0)
        + 6 * js_round(f64::from(g) / 255.0 * 5.0)
        + js_round(f64::from(b) / 255.0 * 5.0)
}

/// JS `Math.round`: half-up, i.e. `floor(x + 0.5)`. Rust's `f64::round` is
/// half-away-from-zero; identical for our non-negative inputs, but the JS
/// spelling keeps the port bit-exact by construction.
#[inline]
fn js_round(value: f64) -> u8 {
    (value + 0.5).floor() as u8
}

/// Maps to ansi-styles `ansi256ToAnsi`: collapses a 256-color index onto the
/// 16-color SGR range (30-37 / 90-97).
/// JS `Math.round` on a 0.0..=1.0 channel value.
#[inline]
fn js_round_f(value: f64) -> u8 {
    (value + 0.5).floor() as u8
}

pub fn ansi256_to_ansi(code: u8) -> u8 {
    if code < 8 {
        return 30 + code;
    }
    if code < 16 {
        return 90 + (code - 8);
    }

    let (r, g, b) = if code >= 232 {
        let v = (f64::from(code - 232) * 10.0 + 8.0) / 255.0;
        (v, v, v)
    } else {
        let c = code - 16;
        let remainder = c % 36;
        (
            f64::from(c / 36) / 5.0,
            f64::from(remainder / 6) / 5.0,
            f64::from(remainder % 6) / 5.0,
        )
    };

    let value = r.max(g).max(b) * 2.0;
    if value == 0.0 {
        return 30;
    }

    let mut result = 30 + ((js_round_f(b) << 2) | (js_round_f(g) << 1) | js_round_f(r));
    if value == 2.0 {
        result += 60;
    }
    result
}

/// Maps to ansi-styles `hexToRgb`: the JS regex `/[a-f\d]{6}|[a-f\d]{3}/i`
/// finds the first contiguous run of 6 (preferred at each position) or 3 hex
/// digits; input without such a run yields black.
pub fn hex_to_rgb(hex: &str) -> (u8, u8, u8) {
    let bytes = hex.as_bytes();
    let all_hex = |range: &[u8]| range.iter().all(u8::is_ascii_hexdigit);
    let candidate = (0..bytes.len()).find_map(|i| {
        if bytes.len() - i >= 6 && all_hex(&bytes[i..i + 6]) {
            Some(&hex[i..i + 6])
        } else if bytes.len() - i >= 3 && all_hex(&bytes[i..i + 3]) {
            Some(&hex[i..i + 3])
        } else {
            None
        }
    });
    let Some(candidate) = candidate else {
        return (0, 0, 0);
    };
    let expanded = if candidate.len() == 3 {
        candidate.chars().flat_map(|c| [c, c]).collect::<String>()
    } else {
        candidate.to_string()
    };
    let value = u32::from_str_radix(&expanded, 16).expect("scanned hex digits");
    (
        ((value >> 16) & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        (value & 0xff) as u8,
    )
}

/// Maps to ansi-styles `rgbToAnsi`.
pub fn rgb_to_ansi(r: u8, g: u8, b: u8) -> u8 {
    ansi256_to_ansi(rgb_to_ansi256(r, g, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_to_ansi256_matches_color_convert_oracle() {
        // Greyscale ramp endpoints and midpoint.
        assert_eq!(rgb_to_ansi256(0, 0, 0), 16);
        assert_eq!(rgb_to_ansi256(255, 255, 255), 231);
        assert_eq!(rgb_to_ansi256(128, 128, 128), 244);
        // Cube corners.
        assert_eq!(rgb_to_ansi256(255, 0, 0), 196);
        assert_eq!(rgb_to_ansi256(0, 255, 0), 46);
        assert_eq!(rgb_to_ansi256(0, 0, 255), 21);
        // Claude orange rgb(215,119,87); node ansi-styles oracle says 174.
        assert_eq!(rgb_to_ansi256(215, 119, 87), 174);
    }

    #[test]
    fn ansi256_to_ansi_matches_oracle() {
        // Direct 16-color passthrough.
        assert_eq!(ansi256_to_ansi(1), 31);
        assert_eq!(ansi256_to_ansi(9), 91);
        // Cube red 196 → bright red.
        assert_eq!(ansi256_to_ansi(196), 91);
        // Black cube corner.
        assert_eq!(ansi256_to_ansi(16), 30);
        // Greyscale: dark → black; 255 (grey93) rounds to plain white, and
        // only a full value of 2 (e.g. cube white 231) gets the bright offset.
        assert_eq!(ansi256_to_ansi(232), 30);
        assert_eq!(ansi256_to_ansi(255), 37);
        assert_eq!(ansi256_to_ansi(231), 97);
        assert_eq!(ansi256_to_ansi(240), 30);
    }

    #[test]
    fn hex_parsing_matches_oracle() {
        assert_eq!(hex_to_rgb("#ff0000"), (255, 0, 0));
        assert_eq!(hex_to_rgb("00ff00"), (0, 255, 0));
        assert_eq!(hex_to_rgb("#f0a"), (255, 0, 170));
        assert_eq!(hex_to_rgb("nope"), (0, 0, 0));
        // The JS regex matches the first CONTIGUOUS 6- or 3-digit run; node
        // chalk v6 oracle for each of these:
        assert_eq!(hex_to_rgb("0xFF00FF"), (255, 0, 255));
        assert_eq!(hex_to_rgb("#abcdef12"), (171, 205, 239));
        assert_eq!(hex_to_rgb("zzab12cd34"), (171, 18, 205));
        assert_eq!(hex_to_rgb("12x345"), (51, 68, 85));
        assert_eq!(hex_to_rgb("ab"), (0, 0, 0));
    }
}

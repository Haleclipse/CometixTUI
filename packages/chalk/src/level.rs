//! Color-support detection.
//!
//! Maps to: chalk/source/vendor/supports-color/index.js, plus the two
//! Claude Code Ink adjustments from `ink/colorize.ts`
//! (`boostChalkLevelForXtermJs`, `clampChalkLevelForTmux`).

use std::io::IsTerminal;
use std::sync::OnceLock;

/// Terminal color support level, mirroring chalk/supports-color:
///
/// - `0`: colors disabled
/// - `1`: basic 16-color support
/// - `2`: 256-color support
/// - `3`: truecolor (16 million colors)
pub type ColorLevel = u8;

/// Maps to supports-color's `hasFlag` (vendored from has-flag): a flag matches
/// only before a `--` terminator.
fn has_flag(args: &[String], flag: &str) -> bool {
    let prefix = if flag.starts_with('-') {
        ""
    } else if flag.len() == 1 {
        "-"
    } else {
        "--"
    };
    let needle = format!("{prefix}{flag}");
    args.iter()
        .take_while(|arg| arg.as_str() != "--")
        .any(|arg| *arg == needle)
}

/// Maps to JS `Number.parseInt(value, 10)` for the FORCE_COLOR /
/// TERM_PROGRAM_VERSION parses: leading whitespace is skipped, an optional
/// sign and leading digits are consumed, and anything else yields NaN (`None`).
fn js_parse_int(value: &str) -> Option<i64> {
    let value = value.trim_start_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    let mut end = 0;
    for (index, c) in value.char_indices() {
        if c.is_ascii_digit() || (index == 0 && matches!(c, '+' | '-')) {
            end = index + c.len_utf8();
        } else {
            break;
        }
    }
    value[..end].parse::<i64>().ok()
}

/// Maps to chalk v6 `envForceColor`: `false` → 0, `true`/empty → 1, a purely
/// numeric value → that level clamped to 3, anything else → unset (detection
/// continues as if FORCE_COLOR were absent).
fn env_force_color(mut env: impl FnMut(&str) -> Option<String>) -> Option<u8> {
    let force = env("FORCE_COLOR")?;
    match force.as_str() {
        "false" => Some(0),
        "true" | "" => Some(1),
        other if other.bytes().all(|b| b.is_ascii_digit()) => {
            js_parse_int(other).map(|n| n.clamp(0, 3) as u8)
        }
        _ => None,
    }
}

/// Detects the color level exactly like chalk's vendored `_supportsColor`,
/// then applies the CC Ink adjustments. All inputs are injected for tests.
pub(crate) fn detect_with_env(
    mut env: impl FnMut(&str) -> Option<String>,
    args: &[String],
    stream_is_tty: bool,
) -> ColorLevel {
    let flag_force: Option<u8> = if ["no-color", "no-colors", "color=false", "color=never"]
        .iter()
        .any(|f| has_flag(args, f))
    {
        Some(0)
    } else if ["color", "colors", "color=true", "color=always"]
        .iter()
        .any(|f| has_flag(args, f))
    {
        Some(1)
    } else {
        None
    };
    let force_color = env_force_color(&mut env).or(flag_force);

    let base = supports_color_base(&mut env, args, stream_is_tty, force_color);

    // Maps to ink/colorize.ts: boost first so the tmux clamp can re-clamp when
    // tmux runs inside a VS Code terminal. The boost requires exactly level 2
    // so NO_COLOR / FORCE_COLOR=0 (level 0) stay respected.
    let boosted = if base == 2 && env("TERM_PROGRAM").as_deref() == Some("vscode") {
        3
    } else {
        base
    };
    if boosted > 2
        && env("TMUX").is_some_and(|v| !v.is_empty())
        && !env("CLAUDE_CODE_TMUX_TRUECOLOR").is_some_and(|v| !v.is_empty())
    {
        2
    } else {
        boosted
    }
}

fn supports_color_base(
    mut env: impl FnMut(&str) -> Option<String>,
    args: &[String],
    stream_is_tty: bool,
    force_color: Option<u8>,
) -> ColorLevel {
    if force_color == Some(0) {
        return 0;
    }

    if ["color=16m", "color=full", "color=truecolor"]
        .iter()
        .any(|f| has_flag(args, f))
    {
        return 3;
    }
    if has_flag(args, "color=256") {
        return 2;
    }

    // Azure DevOps pipelines; checked above the TTY test.
    if env("TF_BUILD").is_some() && env("AGENT_NAME").is_some() {
        return 1;
    }

    if !stream_is_tty && force_color.is_none() {
        return 0;
    }

    let min = force_color.unwrap_or(0);

    if env("TERM").as_deref() == Some("dumb") {
        return min;
    }

    // The Windows OS-build probe is intentionally simplified: modern Windows
    // 10/11 consoles all pass the 14931 truecolor threshold chalk checks for.
    #[cfg(windows)]
    {
        return 3;
    }

    #[cfg(not(windows))]
    {
        if env("CI").is_some() {
            if ["GITHUB_ACTIONS", "GITEA_ACTIONS", "CIRCLECI"]
                .iter()
                .any(|k| env(k).is_some())
            {
                return 3;
            }
            if ["TRAVIS", "APPVEYOR", "GITLAB_CI", "BUILDKITE", "DRONE"]
                .iter()
                .any(|k| env(k).is_some())
                || env("CI_NAME").as_deref() == Some("codeship")
            {
                return 1;
            }
            return min;
        }

        if let Some(version) = env("TEAMCITY_VERSION") {
            return u8::from(teamcity_supports_color(&version));
        }

        if env("COLORTERM").as_deref() == Some("truecolor") {
            return 3;
        }

        if matches!(
            env("TERM").as_deref(),
            Some("xterm-kitty" | "xterm-ghostty" | "wezterm")
        ) {
            return 3;
        }

        if let Some(term_program) = env("TERM_PROGRAM") {
            let version = env("TERM_PROGRAM_VERSION")
                .and_then(|v| js_parse_int(v.split('.').next().unwrap_or_default()));
            match term_program.as_str() {
                "iTerm.app" => {
                    return if version.is_some_and(|v| v >= 3) {
                        3
                    } else {
                        2
                    }
                }
                "Apple_Terminal" => return 2,
                _ => {}
            }
        }

        let term = env("TERM").unwrap_or_default();
        let term_lower = term.to_ascii_lowercase();
        if term_lower.ends_with("-256color") || term_lower.ends_with("-256") {
            return 2;
        }
        if term_lower.starts_with("screen")
            || term_lower.starts_with("xterm")
            || term_lower.starts_with("vt100")
            || term_lower.starts_with("vt220")
            || term_lower.starts_with("rxvt")
            || term_lower.contains("color")
            || term_lower.contains("ansi")
            || term_lower.contains("cygwin")
            || term_lower.contains("linux")
        {
            return 1;
        }

        if env("COLORTERM").is_some() {
            return 1;
        }

        min
    }
}

/// Maps to supports-color's TeamCity regex `^(9\.(0*[1-9]\d*)\.|\d{2,}\.)`:
/// true for 9.x (x >= 1) and any two-or-more digit major version.
fn teamcity_supports_color(version: &str) -> bool {
    let mut parts = version.split('.');
    let Some(major) = parts.next() else {
        return false;
    };
    if major == "9" {
        return parts.next().is_some_and(|minor| {
            minor
                .trim_start_matches('0')
                .parse::<u64>()
                .is_ok_and(|n| n > 0)
        });
    }
    major.len() >= 2 && major.chars().all(|c| c.is_ascii_digit())
}

/// Process-wide stdout color level, computed once. Mirrors chalk's singleton
/// `chalk.level` after CC Ink's colorize.ts adjustments.
pub fn stdout_level() -> ColorLevel {
    static LEVEL: OnceLock<ColorLevel> = OnceLock::new();
    *LEVEL.get_or_init(|| {
        detect_with_env(
            |key| std::env::var(key).ok(),
            &std::env::args().collect::<Vec<_>>(),
            std::io::stdout().is_terminal(),
        )
    })
}

/// Process-wide stderr color level, computed once. Mirrors `chalkStderr`.
pub fn stderr_level() -> ColorLevel {
    static LEVEL: OnceLock<ColorLevel> = OnceLock::new();
    *LEVEL.get_or_init(|| {
        detect_with_env(
            |key| std::env::var(key).ok(),
            &std::env::args().collect::<Vec<_>>(),
            std::io::stderr().is_terminal(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(pairs: &[(&str, &str)], args: &[&str], tty: bool) -> u8 {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        detect_with_env(
            |key| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| v.to_string())
            },
            &args,
            tty,
        )
    }

    #[test]
    fn force_color_overrides_everything() {
        assert_eq!(level(&[("FORCE_COLOR", "0")], &[], true), 0);
        assert_eq!(level(&[("FORCE_COLOR", "3")], &[], false), 3);
        assert_eq!(level(&[("FORCE_COLOR", "")], &[], false), 1);
        assert_eq!(level(&[("FORCE_COLOR", "true")], &[], false), 1);
        assert_eq!(level(&[("FORCE_COLOR", "false")], &[], true), 0);
        // Values above 3 clamp like JS Math.min(n, 3).
        assert_eq!(level(&[("FORCE_COLOR", "9")], &[], false), 3);
        // chalk v6: non-numeric values are treated as unset, so detection
        // proceeds normally (node oracle: banana + xterm-256color tty → 2).
        assert_eq!(
            level(
                &[("FORCE_COLOR", "banana"), ("TERM", "xterm-256color")],
                &[],
                true
            ),
            2
        );
        assert_eq!(
            level(
                &[("FORCE_COLOR", "banana"), ("TERM", "xterm-256color")],
                &[],
                false
            ),
            0
        );
    }

    #[test]
    fn cli_flags_match_has_flag_semantics() {
        assert_eq!(level(&[], &["--color=truecolor"], false), 3);
        assert_eq!(level(&[], &["--color=256"], false), 2);
        assert_eq!(
            level(&[("TERM", "xterm-256color")], &["--no-color"], true),
            0
        );
        // Flags after the -- terminator are ignored.
        assert_eq!(level(&[], &["--", "--color=truecolor"], false), 0);
    }

    #[test]
    fn non_tty_defaults_to_zero_without_force() {
        assert_eq!(level(&[("TERM", "xterm-256color")], &[], false), 0);
    }

    #[test]
    fn term_detection_matches_chalk_oracle() {
        assert_eq!(level(&[("TERM", "dumb")], &[], true), 0);
        assert_eq!(level(&[("COLORTERM", "truecolor")], &[], true), 3);
        assert_eq!(level(&[("TERM", "xterm-kitty")], &[], true), 3);
        assert_eq!(level(&[("TERM", "xterm-ghostty")], &[], true), 3);
        assert_eq!(level(&[("TERM", "wezterm")], &[], true), 3);
        assert_eq!(
            level(
                &[
                    ("TERM_PROGRAM", "iTerm.app"),
                    ("TERM_PROGRAM_VERSION", "3.5.9")
                ],
                &[],
                true
            ),
            3
        );
        assert_eq!(
            level(
                &[
                    ("TERM_PROGRAM", "iTerm.app"),
                    ("TERM_PROGRAM_VERSION", "2.1")
                ],
                &[],
                true
            ),
            2
        );
        assert_eq!(level(&[("TERM_PROGRAM", "Apple_Terminal")], &[], true), 2);
        assert_eq!(level(&[("TERM", "xterm-256color")], &[], true), 2);
        assert_eq!(level(&[("TERM", "screen-256")], &[], true), 2);
        assert_eq!(level(&[("TERM", "xterm")], &[], true), 1);
        assert_eq!(level(&[("TERM", "vt220")], &[], true), 1);
        assert_eq!(level(&[("COLORTERM", "1")], &[], true), 1);
        assert_eq!(level(&[], &[], true), 0);
    }

    #[test]
    fn ci_detection() {
        // Like chalk, the non-TTY guard fires before the CI branch, so these
        // need a TTY stream (node oracle: createSupportsColor({isTTY:false})
        // is false even with GITHUB_ACTIONS set).
        assert_eq!(level(&[("CI", "1"), ("GITHUB_ACTIONS", "1")], &[], true), 3);
        assert_eq!(level(&[("CI", "1"), ("TRAVIS", "1")], &[], true), 1);
        assert_eq!(level(&[("CI", "1"), ("CI_NAME", "codeship")], &[], true), 1);
        assert_eq!(level(&[("CI", "1")], &[], true), 0);
        // Azure's TF_BUILD check sits above the TTY guard.
        assert_eq!(
            level(&[("TF_BUILD", "1"), ("AGENT_NAME", "x")], &[], false),
            1
        );
        assert_eq!(
            level(&[("CI", "1"), ("GITHUB_ACTIONS", "1")], &[], false),
            0
        );
    }

    #[test]
    fn teamcity_version_gate() {
        assert_eq!(level(&[("TEAMCITY_VERSION", "9.1.5")], &[], true), 1);
        assert_eq!(level(&[("TEAMCITY_VERSION", "9.0.3")], &[], true), 0);
        assert_eq!(level(&[("TEAMCITY_VERSION", "2023.11")], &[], true), 1);
        assert_eq!(level(&[("TEAMCITY_VERSION", "8.1")], &[], true), 0);
    }

    #[test]
    fn vscode_boost_requires_exactly_level_two() {
        // 256-color under vscode boosts to truecolor.
        assert_eq!(
            level(
                &[("TERM", "xterm-256color"), ("TERM_PROGRAM", "vscode")],
                &[],
                true
            ),
            3
        );
        // NO_COLOR-style level 0 must NOT be boosted.
        assert_eq!(
            level(
                &[("FORCE_COLOR", "0"), ("TERM_PROGRAM", "vscode")],
                &[],
                true
            ),
            0
        );
    }

    #[test]
    fn tmux_clamps_truecolor_after_boost() {
        assert_eq!(
            level(
                &[
                    ("COLORTERM", "truecolor"),
                    ("TMUX", "/tmp/tmux-1000/default,123,0")
                ],
                &[],
                true
            ),
            2
        );
        // The escape hatch restores truecolor.
        assert_eq!(
            level(
                &[
                    ("COLORTERM", "truecolor"),
                    ("TMUX", "/tmp/x"),
                    ("CLAUDE_CODE_TMUX_TRUECOLOR", "1")
                ],
                &[],
                true
            ),
            3
        );
        // Boost-then-clamp: tmux inside a VS Code terminal ends at 2.
        assert_eq!(
            level(
                &[
                    ("TERM", "xterm-256color"),
                    ("TERM_PROGRAM", "vscode"),
                    ("TMUX", "/tmp/x")
                ],
                &[],
                true
            ),
            2
        );
    }
}

use crate::{
    canvas::UnderlineStyle,
    components::{MixedText, MixedTextContent},
    render::MeasureFunc,
    segmented_string::SegmentedString,
    strip_ansi::strip_ansi,
    CanvasTextStyle, Color, Component, ComponentDrawer, ComponentUpdater, Hooks, Props, TextStyles,
    Weight,
};
use taffy::{AvailableSpace, Size};

/// The text wrapping behavior of a [`Text`] component.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum TextWrap {
    /// Text is wrapped at appropriate characters to minimize overflow. This is the default.
    #[default]
    Wrap,
    /// Text is wrapped and each visual line is trimmed, matching CC Ink's `wrap-trim` mode.
    WrapTrim,
    /// Text is not wrapped, and may overflow the bounds of the component.
    NoWrap,
    /// CC Ink legacy `end` wrap value. In the current CC Ink fork this is a
    /// no-op in `wrapText(...)`, so it behaves like [`Self::NoWrap`].
    End,
    /// CC Ink legacy `middle` wrap value. In the current CC Ink fork this is a
    /// no-op in `wrapText(...)`, so it behaves like [`Self::NoWrap`].
    Middle,
    /// Text is truncated at the end with an ellipsis, matching CC Ink's `truncate` alias.
    Truncate,
    /// Text is truncated at the end with an ellipsis.
    TruncateEnd,
    /// Text is truncated in the middle with an ellipsis.
    TruncateMiddle,
    /// Text is truncated at the start with an ellipsis.
    TruncateStart,
}

/// The text alignment of a [`Text`] component.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum TextAlign {
    /// Text is aligned to the left. This is the default.
    #[default]
    Left,
    /// Text is aligned to the right.
    Right,
    /// Text is aligned to the center.
    Center,
}

/// The text decoration of a [`Text`] component.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum TextDecoration {
    /// No text decoration. This is the default.
    #[default]
    None,
    /// The text is underlined.
    Underline,
}

/// Maps to CC `ink/squash-text-nodes.ts#StyledSegment:8-12`.
///
/// L1 (`Explicit structured Ink text-flow carrier`): iocraft has no React
/// host DOM to traverse, so translated callers provide ordered child-local
/// segment fields and [`Text`] performs the outer-root merge before drawing.
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StyledSegment {
    /// Plain text contributed by this segment.
    pub text: String,

    /// Optional child-local style fields.
    pub styles: TextStyles,

    /// Optional OSC 8 hyperlink target.
    pub hyperlink: Option<String>,
}

impl StyledSegment {
    /// Creates an unstyled segment with the given text.
    pub fn new(text: impl ToString) -> Self {
        Self {
            text: text.to_string(),
            ..Self::default()
        }
    }
}

/// The props which can be passed to the [`Text`] component.
#[non_exhaustive]
#[derive(Default, Props)]
pub struct TextProps {
    /// The color to make the text.
    pub color: Option<Color>,

    /// The background color to paint behind the rendered text cells.
    pub background_color: Option<Color>,

    /// The content of the text.
    pub content: String,

    /// Explicit structured descendants replacing CC's runtime host-tree
    /// traversal. `None` keeps the legacy string fast path; `Some` is
    /// authoritative even when empty or when [`Self::content`] is non-empty.
    pub segments: Option<Vec<StyledSegment>>,

    /// The weight of the text.
    pub weight: Weight,

    /// CC Ink-style alias for [`Weight::Bold`] (`ink/styles.ts` `bold`).
    ///
    /// Independent of [`Self::dim`]: setting both yields bold *and* dim, which
    /// is what CC Ink produces by applying both chalk wrappers
    /// (`ink/colorize.ts:203-207`).
    pub bold: bool,

    /// CC Ink-style alias for SGR dim (`ink/styles.ts` `dim`).
    ///
    /// Independent of [`Self::bold`]; [`Weight::Light`] is the weight-side
    /// spelling of the same attribute.
    pub dim: bool,

    /// The text wrapping behavior.
    pub wrap: TextWrap,

    /// The text alignment.
    pub align: TextAlign,

    /// The text decoration.
    pub decoration: TextDecoration,

    /// CC Ink-style alias for [`TextDecoration::Underline`].
    pub underline: bool,

    /// Whether to italicize the text.
    pub italic: bool,

    /// Whether to strike through the text.
    pub strikethrough: bool,

    /// Whether to draw an overline above the text.
    pub overline: bool,

    /// Whether to invert the text's foreground and background colors.
    pub invert: bool,

    /// CC Ink-style alias for [`Self::invert`].
    pub inverse: bool,

    /// If set, renders the text as an OSC 8 hyperlink. Terminals with support
    /// (kitty, iTerm2, WezTerm, Windows Terminal, etc.) allow clicking or
    /// Cmd/Ctrl-clicking to open the URL. Unsupported terminals display the
    /// text normally.
    pub href: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TruncatePosition {
    Start,
    Middle,
    End,
}

struct WrappedTextLine {
    text: String,
    soft_continuation: bool,
    content_end: usize,
}

/// `Text` is a component that renders a text string.
///
/// # Example
///
/// ```
/// # use iocraft::prelude::*;
/// # fn my_element() -> impl Into<AnyElement<'static>> {
/// element! {
///     Text(content: "Hello!")
/// }
/// # }
/// ```
#[derive(Default)]
pub struct Text {
    style: CanvasTextStyle,
    background_color: Option<Color>,
    content: String,
    wrap: TextWrap,
    align: TextAlign,
    hyperlink: Option<String>,
    structured: bool,
    mixed_text: MixedText,
}

impl Text {
    pub(crate) fn measure_func(content: String, text_wrap: TextWrap) -> MeasureFunc {
        Box::new(move |known_size, available_space, _| {
            let content = Text::wrap(&content, text_wrap, known_size.width, available_space.width);
            let measured = crate::canvas::measure_text(&content, None);
            Size {
                width: measured.width as _,
                height: measured.height as _,
            }
        })
    }

    fn line_is_soft_continuation(
        content: &str,
        line: &crate::segmented_string::SegmentedStringLine<'_>,
    ) -> bool {
        let Some(first_segment) = line.segments.first() else {
            return false;
        };
        first_segment.index == 0
            && first_segment.offset > 0
            && !content[..first_segment.offset].ends_with('\n')
    }

    fn truncate_position(text_wrap: TextWrap) -> Option<TruncatePosition> {
        match text_wrap {
            TextWrap::Truncate | TextWrap::TruncateEnd => Some(TruncatePosition::End),
            TextWrap::TruncateMiddle => Some(TruncatePosition::Middle),
            TextWrap::TruncateStart => Some(TruncatePosition::Start),
            _ => None,
        }
    }

    fn slice_display_columns(text: &str, start: usize, end: usize) -> String {
        if start >= end {
            return String::new();
        }

        let mut ret = String::new();
        let mut col = 0;
        for grapheme in unicode_segmentation::UnicodeSegmentation::graphemes(text, true) {
            let width = crate::canvas::string_display_width_from_col(grapheme, col);
            let next = col + width;
            if next <= start {
                col = next;
                continue;
            }
            if col >= end {
                break;
            }
            // Match CC Ink's sliceFit behavior: a wide grapheme that straddles
            // the boundary is omitted rather than allowed to overflow the target
            // column range.
            if col >= start && next <= end {
                ret.push_str(grapheme);
            }
            col = next;
        }
        ret
    }

    fn truncate_line(text: &str, columns: usize, position: TruncatePosition) -> String {
        const ELLIPSIS: &str = "…";

        if columns < 1 {
            return String::new();
        }
        if columns == 1 {
            return ELLIPSIS.to_string();
        }

        let width = crate::canvas::string_display_width(text);
        if width <= columns {
            return text.to_string();
        }

        match position {
            TruncatePosition::Start => {
                format!(
                    "{ELLIPSIS}{}",
                    Self::slice_display_columns(text, width - columns + 1, width)
                )
            }
            TruncatePosition::Middle => {
                let prefix_columns = columns / 2;
                let suffix_columns = columns - prefix_columns - 1;
                format!(
                    "{}{}{}",
                    Self::slice_display_columns(text, 0, prefix_columns),
                    ELLIPSIS,
                    Self::slice_display_columns(text, width - suffix_columns, width)
                )
            }
            TruncatePosition::End => {
                format!(
                    "{}{ELLIPSIS}",
                    Self::slice_display_columns(text, 0, columns - 1)
                )
            }
        }
    }

    fn wrap_lines(content: &str, text_wrap: TextWrap, width: usize) -> Vec<WrappedTextLine> {
        if let Some(position) = Self::truncate_position(text_wrap) {
            return content
                .split('\n')
                .map(|line| {
                    let text = Self::truncate_line(line, width, position);
                    let content_end = crate::canvas::string_display_width(&text);
                    WrappedTextLine {
                        text,
                        soft_continuation: false,
                        content_end,
                    }
                })
                .collect();
        }

        match text_wrap {
            TextWrap::NoWrap | TextWrap::End | TextWrap::Middle => content
                .split('\n')
                .map(|line| WrappedTextLine {
                    text: line.to_string(),
                    soft_continuation: false,
                    content_end: crate::canvas::string_display_width(line),
                })
                .collect(),
            TextWrap::Wrap | TextWrap::WrapTrim => {
                let trimmed_content;
                let source = if text_wrap == TextWrap::WrapTrim {
                    trimmed_content = content
                        .split('\n')
                        .map(str::trim)
                        .collect::<Vec<_>>()
                        .join("\n");
                    trimmed_content.as_str()
                } else {
                    content
                };
                let segmented: SegmentedString = source.into();
                let lines = segmented.wrap(width);
                let soft_continuations = lines
                    .iter()
                    .map(|line| Self::line_is_soft_continuation(source, line))
                    .collect::<Vec<_>>();
                let line_count = lines.len();
                lines
                    .into_iter()
                    .enumerate()
                    .map(|(index, line)| {
                        let preserve_content_end =
                            index + 1 < line_count && soft_continuations[index + 1];
                        let (text, rendered_width) = if text_wrap == TextWrap::WrapTrim {
                            let text = line.to_string().trim().to_string();
                            let width = crate::canvas::string_display_width(&text);
                            (text, width)
                        } else {
                            let mut trimmed = line.clone();
                            trimmed.trim_end();
                            (trimmed.to_string(), trimmed.width)
                        };
                        let content_end = if preserve_content_end {
                            line.width
                        } else {
                            rendered_width
                        };
                        WrappedTextLine {
                            text,
                            soft_continuation: soft_continuations[index],
                            content_end,
                        }
                    })
                    .collect()
            }
            TextWrap::Truncate
            | TextWrap::TruncateEnd
            | TextWrap::TruncateMiddle
            | TextWrap::TruncateStart => unreachable!("handled above"),
        }
    }

    fn wrap_to_string(s: &str, text_wrap: TextWrap, width: usize) -> String {
        Self::wrap_lines(s, text_wrap, width)
            .into_iter()
            .map(|line| line.text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn wrap(
        content: &str,
        text_wrap: TextWrap,
        known_width: Option<f32>,
        available_width: AvailableSpace,
    ) -> String {
        match text_wrap {
            TextWrap::Wrap
            | TextWrap::WrapTrim
            | TextWrap::Truncate
            | TextWrap::TruncateEnd
            | TextWrap::TruncateMiddle
            | TextWrap::TruncateStart => match known_width {
                Some(w) => Self::wrap_to_string(content, text_wrap, w as usize),
                None => match available_width {
                    AvailableSpace::Definite(w) => {
                        Self::wrap_to_string(content, text_wrap, w as usize)
                    }
                    AvailableSpace::MaxContent => content.to_string(),
                    AvailableSpace::MinContent => Self::wrap_to_string(content, text_wrap, 1),
                },
            },
            TextWrap::NoWrap | TextWrap::End | TextWrap::Middle => content.to_string(),
        }
    }

    pub(crate) fn alignment_padding(line_width: usize, align: TextAlign, width: usize) -> isize {
        match align {
            TextAlign::Left => 0,
            TextAlign::Right => width as isize - line_width as isize,
            TextAlign::Center => width as isize / 2 - line_width as isize / 2,
        }
    }
}

/// Wraps or truncates text using the same modes as the [`Text`] component.
///
/// This is the Rust counterpart to CC Ink's exported `wrapText(...)` helper.
/// `TextWrap::Wrap` and `TextWrap::WrapTrim` hard-wrap using terminal display
/// width, while the truncate modes return a single ellipsized line per input
/// line. `TextWrap::NoWrap`, `TextWrap::End`, and `TextWrap::Middle` return
/// the input unchanged, matching the current CC Ink `wrapText(...)` helper.
pub fn wrap_text(text: &str, max_width: usize, wrap: TextWrap) -> String {
    if matches!(wrap, TextWrap::NoWrap | TextWrap::End | TextWrap::Middle) {
        return text.to_string();
    }
    Text::wrap_to_string(text, wrap, max_width)
}

pub(crate) struct TextDrawer<'a, 'b> {
    x_offset: isize,
    x: isize,
    y: isize,
    drawer: &'a mut ComponentDrawer<'b>,
    line_encountered_non_whitespace: bool,
    skip_leading_whitespace: bool,
    prev_line_content_end: usize,
}

impl<'a, 'b> TextDrawer<'a, 'b> {
    pub fn new(
        drawer: &'a mut ComponentDrawer<'b>,
        x_offset: isize,
        skip_leading_whitespace: bool,
    ) -> Self {
        TextDrawer {
            x_offset,
            x: x_offset,
            y: 0,
            drawer,
            line_encountered_non_whitespace: false,
            skip_leading_whitespace,
            prev_line_content_end: 0,
        }
    }

    pub fn append_lines<'c>(
        &mut self,
        lines: impl IntoIterator<Item = &'c str>,
        style: CanvasTextStyle,
    ) {
        self.append_lines_with_link(lines, style, None);
    }

    pub fn append_lines_with_link<'c>(
        &mut self,
        lines: impl IntoIterator<Item = &'c str>,
        style: CanvasTextStyle,
        hyperlink: Option<&str>,
    ) {
        self.append_lines_with_background_and_link(lines, style, None, hyperlink);
    }

    pub(crate) fn append_lines_with_background_and_link<'c>(
        &mut self,
        lines: impl IntoIterator<Item = &'c str>,
        style: CanvasTextStyle,
        background_color: Option<Color>,
        hyperlink: Option<&str>,
    ) {
        self.append_lines_with_soft_wrap_with_link(
            lines.into_iter().map(|line| (line, false)),
            style,
            background_color,
            hyperlink,
        );
    }

    pub(crate) fn mark_current_line_soft_wrap(&mut self) {
        self.drawer
            .canvas()
            .mark_soft_wrap_continuation(self.y, self.prev_line_content_end);
    }

    pub(crate) fn finish_line(&mut self) {
        self.y += 1;
        self.x = self.x_offset;
        self.line_encountered_non_whitespace = false;
    }

    pub(crate) fn set_prev_line_content_end(&mut self, content_end: usize) {
        self.prev_line_content_end = content_end;
    }

    fn append_lines_with_soft_wrap_with_link<'c>(
        &mut self,
        lines: impl IntoIterator<Item = (&'c str, bool)>,
        style: CanvasTextStyle,
        background_color: Option<Color>,
        hyperlink: Option<&str>,
    ) {
        let mut lines = lines.into_iter().peekable();
        while let Some((mut line, soft_continuation)) = lines.next() {
            if soft_continuation {
                self.drawer
                    .canvas()
                    .mark_soft_wrap_continuation(self.y, self.prev_line_content_end);
            }
            if self.skip_leading_whitespace && !self.line_encountered_non_whitespace {
                let to_skip = line
                    .chars()
                    .position(|c| !c.is_whitespace())
                    .unwrap_or(line.len());
                let (whitespace, remaining) = line.split_at(to_skip);
                self.x += crate::canvas::string_display_width_from_col(
                    whitespace,
                    self.x.max(0) as usize,
                ) as isize;
                line = remaining;
                if !line.is_empty() {
                    self.line_encountered_non_whitespace = true;
                }
            }
            let visual_line = crate::bidi::reorder_bidi_text_for_terminal(line);
            let line = visual_line.as_ref();
            let line_width =
                crate::canvas::string_display_width_from_col(line, self.x.max(0) as usize);
            if let Some(color) = background_color {
                self.drawer
                    .canvas()
                    .set_background_color(self.x, self.y, line_width, 1, color);
            }
            self.drawer
                .canvas()
                .set_text_with_link(self.x, self.y, line, style, hyperlink);
            let line_end = (self.x + line_width as isize).max(0);
            self.prev_line_content_end = line_end as usize;
            if lines.peek().is_some() {
                self.finish_line();
            } else {
                self.x += line_width as isize;
            }
        }
    }
}

impl Component for Text {
    type Props<'a> = TextProps;

    fn new(_props: &Self::Props<'_>) -> Self {
        Self::default()
    }

    fn update(
        &mut self,
        props: &mut Self::Props<'_>,
        _hooks: Hooks,
        updater: &mut ComponentUpdater,
    ) {
        self.style = CanvasTextStyle {
            color: props.color,
            // `bold` and `dim` are independent attributes in CC Ink, so `dim`
            // gets its own flag and no longer erases `bold`. `Weight::Light`
            // stays the weight-side spelling for dim-only text, which is what
            // `MixedTextContent` and direct `weight:` callers use.
            weight: if props.bold {
                Weight::Bold
            } else if props.dim {
                Weight::Light
            } else {
                props.weight
            },
            dim: props.dim,
            underline: props.underline || props.decoration == TextDecoration::Underline,
            underline_style: UnderlineStyle::Single,
            underline_color: None,
            italic: props.italic,
            blink: false,
            hidden: false,
            strikethrough: props.strikethrough,
            overline: props.overline,
            invert: props.invert || props.inverse,
        };
        self.background_color = props.background_color;
        self.hyperlink = props.href.clone();
        self.wrap = props.wrap;
        self.align = props.align;

        if let Some(segments) = props.segments.as_ref() {
            let root_bold = self.style.weight == Weight::Bold;
            let root_dim = self.style.dim || self.style.weight == Weight::Light;
            let mut contents = Vec::with_capacity(segments.len());
            for segment in segments.iter().filter(|segment| !segment.text.is_empty()) {
                let bold = segment.styles.bold.unwrap_or(root_bold);
                let dim = segment.styles.dim.unwrap_or(root_dim);
                contents.push(MixedTextContent {
                    text: segment.text.clone(),
                    color: segment.styles.color.or(self.style.color),
                    background_color: segment.styles.background_color.or(self.background_color),
                    weight: if bold {
                        Weight::Bold
                    } else if dim {
                        Weight::Light
                    } else {
                        Weight::Normal
                    },
                    dim,
                    decoration: if segment.styles.underline.unwrap_or(self.style.underline) {
                        TextDecoration::Underline
                    } else {
                        TextDecoration::None
                    },
                    italic: segment.styles.italic.unwrap_or(self.style.italic),
                    strikethrough: segment
                        .styles
                        .strikethrough
                        .unwrap_or(self.style.strikethrough),
                    overline: self.style.overline,
                    invert: segment.styles.inverse.unwrap_or(self.style.invert),
                    href: segment
                        .hyperlink
                        .as_ref()
                        .filter(|href| !href.is_empty())
                        .cloned()
                        .or_else(|| self.hyperlink.clone()),
                });
            }
            self.structured = true;
            self.mixed_text
                .update_contents(&mut contents, props.wrap, props.align, updater);
            return;
        }

        self.structured = false;
        self.content = strip_ansi(&props.content).into_owned();
        updater.set_measure_func(Self::measure_func(self.content.clone(), props.wrap));
    }

    fn draw(&mut self, drawer: &mut ComponentDrawer<'_>) {
        if self.structured {
            <MixedText as Component>::draw(&mut self.mixed_text, drawer);
            return;
        }
        if drawer.zero_height_sibling_shares_y() {
            return;
        }
        let layout_width = drawer.layout().size.width;
        let width = if matches!(
            self.wrap,
            TextWrap::NoWrap | TextWrap::End | TextWrap::Middle
        ) {
            layout_width
        } else {
            layout_width.min(drawer.remaining_canvas_size().width as f32)
        };
        let wrapped_lines = Self::wrap_lines(&self.content, self.wrap, width as usize);
        let paddings = wrapped_lines
            .iter()
            .map(|line| {
                Self::alignment_padding(
                    crate::canvas::string_display_width(&line.text),
                    self.align,
                    width as _,
                )
            })
            .collect::<Vec<_>>();
        let x_offset = paddings.iter().copied().min().unwrap_or(0);
        let line_count = wrapped_lines.len();
        let mut drawer = TextDrawer::new(drawer, x_offset, self.align != TextAlign::Left);
        for (index, line) in wrapped_lines.into_iter().enumerate() {
            if line.soft_continuation {
                drawer.mark_current_line_soft_wrap();
            }
            let padding = paddings[index];
            let additional_padding = padding - x_offset;
            if additional_padding > 0 {
                drawer.append_lines(
                    [format!("{:width$}", "", width = additional_padding as usize).as_str()],
                    CanvasTextStyle::default(),
                );
            }
            drawer.append_lines_with_background_and_link(
                [line.text.as_str()],
                self.style,
                self.background_color,
                self.hyperlink.as_deref(),
            );
            drawer.set_prev_line_content_end((padding + line.content_end as isize).max(0) as usize);
            if index + 1 < line_count {
                drawer.finish_line();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::prelude::*;
    use crossterm::{csi, style::Attribute};
    use futures::StreamExt;
    use std::io::Write;

    #[test]
    fn test_wrap_text_helper_matches_text_modes() {
        assert_eq!(wrap_text("abcdef", 5, TextWrap::TruncateEnd), "abcd…");
        assert_eq!(wrap_text("abcdef", 5, TextWrap::TruncateStart), "…cdef");
        assert_eq!(wrap_text("abcdef", 5, TextWrap::TruncateMiddle), "ab…ef");
        assert_eq!(wrap_text("  abc def", 4, TextWrap::WrapTrim), "abc\ndef");
        assert_eq!(wrap_text("abcdef", 3, TextWrap::NoWrap), "abcdef");
        assert_eq!(wrap_text("abcdef", 3, TextWrap::End), "abcdef");
        assert_eq!(wrap_text("abcdef", 3, TextWrap::Middle), "abcdef");
    }

    /// Maps to CC `ink/squash-text-nodes.ts:18-63` and
    /// `ink/render-node-to-output.ts:550-626`.
    #[test]
    fn structured_segments_wrap_and_keep_link_boundaries_like_official() {
        let canvas = element! {
            View(width: 7) {
                Text(segments: Some(vec![
                    StyledSegment::new("("),
                    StyledSegment {
                        text: "notes.ipynb".to_string(),
                        hyperlink: Some("file:///notes.ipynb".to_string()),
                        ..StyledSegment::default()
                    },
                    StyledSegment::new("@cell)"),
                ]))
            }
        }
        .render(None);

        let mut visible = String::new();
        let mut linked = String::new();
        for row in 0..canvas.height() {
            for col in 0..canvas.width() {
                let Some(cell) = canvas.cell(col, row) else {
                    continue;
                };
                if let Some(text) = cell.text() {
                    visible.push_str(text);
                    if cell.hyperlink() == Some("file:///notes.ipynb") {
                        linked.push_str(text);
                    }
                }
            }
        }

        assert_eq!(visible, "(notes.ipynb@cell)", "canvas=\n{canvas}");
        assert_eq!(linked, "notes.ipynb");
        assert!(canvas.height() > 1, "canvas=\n{canvas}");
        assert!(
            (1..canvas.height()).any(|row| canvas.soft_wrap_continuation(row) > 0),
            "structured wrapping must retain soft-wrap selection metadata"
        );
    }

    /// Maps to CC `ink/squash-text-nodes.ts:23-26` field-wise style spread.
    #[test]
    fn structured_segment_styles_inherit_and_override_like_official() {
        let canvas = element! {
            Text(
                dim: true,
                segments: Some(vec![
                    StyledSegment::new("outer "),
                    StyledSegment {
                        text: "inner".to_string(),
                        styles: TextStyles {
                            color: Some(Color::Green),
                            background_color: Some(Color::Blue),
                            bold: Some(true),
                            italic: Some(true),
                            underline: Some(true),
                            strikethrough: Some(true),
                            inverse: Some(true),
                            ..TextStyles::default()
                        },
                        hyperlink: None,
                    },
                ]),
            )
        }
        .render(None);

        let outer = canvas.resolved_text_style(0, 0).expect("outer style");
        assert!(outer.is_dim());
        assert!(!outer.is_bold());

        let inner = canvas.resolved_text_style(6, 0).expect("inner style");
        assert_eq!(inner.color, Some(Color::Green));
        assert_eq!(inner.weight, Weight::Bold);
        assert!(inner.dim);
        assert!(inner.italic);
        assert!(inner.underline);
        assert!(inner.strikethrough);
        assert!(inner.invert);
        assert_eq!(
            canvas.cell(6, 0).unwrap().background_color,
            Some(Color::Blue)
        );
    }

    /// Maps to raw host-level `textStyles` object spread in
    /// CC `ink/squash-text-nodes.ts:23-26`.
    #[test]
    fn structured_segment_explicit_false_clears_inherited_style_like_official() {
        let canvas = element! {
            Text(
                bold: true,
                dim: true,
                italic: true,
                segments: Some(vec![StyledSegment {
                    text: "plain".to_string(),
                    styles: TextStyles {
                        bold: Some(false),
                        dim: Some(false),
                        italic: Some(false),
                        ..TextStyles::default()
                    },
                    hyperlink: None,
                }]),
            )
        }
        .render(None);

        let style = canvas.resolved_text_style(0, 0).expect("plain style");
        assert_eq!(style.weight, Weight::Normal);
        assert!(!style.dim);
        assert!(!style.italic);
    }

    /// Maps to CC `ink/squash-text-nodes.ts:51-59` `href || inheritedHyperlink`.
    #[test]
    fn structured_segment_hyperlinks_inherit_and_replace_like_official() {
        let canvas = element! {
            Text(
                href: "https://parent.example".to_string(),
                segments: Some(vec![
                    StyledSegment::new("a"),
                    StyledSegment {
                        text: "b".to_string(),
                        hyperlink: Some(String::new()),
                        ..StyledSegment::default()
                    },
                    StyledSegment {
                        text: "c".to_string(),
                        hyperlink: Some("https://child.example".to_string()),
                        ..StyledSegment::default()
                    },
                ]),
            )
        }
        .render(None);

        assert_eq!(
            canvas.hyperlink_at(0, 0).as_deref(),
            Some("https://parent.example")
        );
        assert_eq!(
            canvas.hyperlink_at(1, 0).as_deref(),
            Some("https://parent.example")
        );
        assert_eq!(
            canvas.hyperlink_at(2, 0).as_deref(),
            Some("https://child.example")
        );
    }

    /// Maps to CC `Text`'s absent children and `squashTextNodesToSegments`'s
    /// empty-text omission while preserving iocraft's legacy string carrier.
    #[test]
    fn structured_segment_presence_and_legacy_content_match_official() {
        assert_eq!(
            element!(Text(
                content: "ignored".to_string(),
                segments: Some(Vec::new())
            ))
            .to_string(),
            ""
        );
        assert_eq!(
            element!(Text(segments: Some(vec![StyledSegment::new("")]))).to_string(),
            ""
        );
        assert_eq!(element!(Text(content: "legacy")).to_string(), "legacy\n");
    }

    #[test]
    fn test_text() {
        assert_eq!(element!(Text).to_string(), "");

        assert_eq!(element!(Text(content: "foo")).to_string(), "foo\n");

        assert_eq!(
            element!(Text(content: "foo\nbar")).to_string(),
            "foo\nbar\n"
        );

        assert_eq!(element!(Text(content: "foo\n")).to_string(), "foo\n\n");
        assert_eq!(
            element! {
                View(flex_direction: FlexDirection::Column) {
                    Text(content: "A")
                    Text(content: "")
                    Text(content: "B")
                }
            }
            .to_string(),
            "A\nB\n"
        );
        assert_eq!(
            element!(Text(content: "foo\n", wrap: TextWrap::NoWrap)).to_string(),
            "foo\n\n"
        );

        assert_eq!(element!(Text(content: "😀")).to_string(), "😀\n");

        assert_eq!(
            element! {
                View(width: 14) {
                    Text(content: "this is a wrapping test")
                }
            }
            .to_string(),
            "this is a\nwrapping test\n"
        );

        assert_eq!(
            element! {
                View(width: 2) {
                    Text(content: "☀️x")
                }
            }
            .to_string(),
            "☀️\nx\n"
        );

        assert_eq!(
            element! {
                View(width: 5, height: 2) {
                    View(width: 10) {
                        Text(content: "abcdef")
                    }
                }
            }
            .render(Some(5))
            .to_string(),
            "abcde\nf\n"
        );

        assert_eq!(
            element! {
                View(width: 15) {
                    Text(content: "this is an alignment test", align: TextAlign::Right)
                }
            }
            .to_string(),
            "     this is an\n alignment test\n"
        );

        assert_eq!(
            element! {
                View(width: 15) {
                    Text(content: "this is an alignment test", align: TextAlign::Center)
                }
            }
            .to_string(),
            "  this is an\nalignment test\n"
        );

        {
            let canvas = element!(Text(content: "bold", bold: true)).render(None);
            assert_eq!(
                canvas.resolved_text_style(0, 0).unwrap().weight,
                Weight::Bold
            );
        }

        {
            // CC Ink applies both wrappers (`ink/colorize.ts:203-207`), so the
            // two attributes coexist rather than one overriding the other.
            let canvas = element!(Text(content: "dim", bold: true, dim: true)).render(None);
            let style = canvas.resolved_text_style(0, 0).unwrap();
            assert_eq!(style.weight, Weight::Bold);
            assert!(style.dim);
        }

        {
            let canvas = element!(Text(content: "under", underline: true)).render(None);
            assert!(canvas.resolved_text_style(0, 0).unwrap().underline);
        }

        {
            let canvas = element!(Text(content: "strike", strikethrough: true)).render(None);
            assert!(canvas.resolved_text_style(0, 0).unwrap().strikethrough);
        }

        {
            let canvas = element!(Text(content: "over", overline: true)).render(None);
            assert!(canvas.resolved_text_style(0, 0).unwrap().overline);
        }

        {
            let canvas = element!(Text(content: "inverse", inverse: true)).render(None);
            assert!(canvas.resolved_text_style(0, 0).unwrap().invert);
        }

        {
            let canvas =
                element!(Text(content: "bg", background_color: Some(Color::Blue))).render(None);
            assert_eq!(
                canvas.cell(0, 0).unwrap().background_color,
                Some(Color::Blue)
            );
            assert_eq!(
                canvas.cell(1, 0).unwrap().background_color,
                Some(Color::Blue)
            );
        }

        assert_eq!(
            element! {
                View(width: 5) {
                    Text(content: "abcdef", wrap: TextWrap::Truncate)
                }
            }
            .to_string(),
            "abcd…\n"
        );
        assert_eq!(
            element! {
                View(width: 5) {
                    Text(content: "abcdef", wrap: TextWrap::TruncateStart)
                }
            }
            .to_string(),
            "…cdef\n"
        );
        assert_eq!(
            element! {
                View(width: 5) {
                    Text(content: "abcdef", wrap: TextWrap::TruncateMiddle)
                }
            }
            .to_string(),
            "ab…ef\n"
        );
        assert_eq!(
            element! {
                View(width: 4) {
                    Text(content: "界abc", wrap: TextWrap::TruncateEnd)
                }
            }
            .to_string(),
            "界a…\n"
        );
        assert_eq!(
            element! {
                View(width: 8) {
                    Text(content: "a\tb")
                }
            }
            .to_string(),
            "a\nb\n"
        );
        assert_eq!(
            element! {
                View(width: 4) {
                    Text(content: "  abc def", wrap: TextWrap::WrapTrim)
                }
            }
            .to_string(),
            "abc\ndef\n"
        );

        // Make sure that when the text is not left-aligned, leading whitespace is not underlined.
        {
            let canvas = element! {
                View(width: 16) {
                    Text(content: "this is an alignment test", align: TextAlign::Center, decoration: TextDecoration::Underline)
                }
            }
            .render(None);
            let mut actual = Vec::new();
            canvas.write_ansi(&mut actual).unwrap();

            let mut expected = Vec::new();
            // row 0
            write!(expected, csi!("0m")).unwrap();
            write!(expected, "   ").unwrap();
            write!(expected, csi!("{}m"), Attribute::Underlined.sgr()).unwrap();
            write!(expected, "this is an").unwrap();
            // Full SGR reset before CSI K so underline doesn't bleed on kitty.
            write!(expected, csi!("0m")).unwrap();
            write!(expected, csi!("K")).unwrap();
            write!(expected, csi!("0m")).unwrap();
            write!(expected, "\r\n").unwrap();
            // row 1
            write!(expected, " ").unwrap();
            write!(expected, csi!("{}m"), Attribute::Underlined.sgr()).unwrap();
            write!(expected, "alignment test").unwrap();
            write!(expected, csi!("0m")).unwrap();
            write!(expected, csi!("K")).unwrap();
            write!(expected, csi!("0m")).unwrap();
            write!(expected, "\r\n").unwrap();

            assert_eq!(actual, expected);
        }
    }

    #[component]
    fn WrappedTextSoftWrapApp(hooks: Hooks) -> impl Into<AnyElement<'static>> {
        let mut system = hooks.use_context_mut::<SystemContext>();
        system.exit();
        element! {
            View(width: 5) {
                Text(content: "hello world")
            }
        }
    }

    #[test]
    fn test_text_marks_soft_wrap_for_selection_copy() {
        let canvases: Vec<_> = smol::block_on(
            element!(WrappedTextSoftWrapApp)
                .mock_terminal_render_loop(MockTerminalConfig::default())
                .collect(),
        );
        assert_eq!(canvases.len(), 1);
        assert_eq!(canvases[0].to_string(), "hello\nworld\n");
        assert_eq!(canvases[0].soft_wrap_continuation(1), 5);
        assert_eq!(
            canvases[0].selected_text(SelectionRange::new(
                SelectionPoint { col: 0, row: 0 },
                SelectionPoint { col: 4, row: 1 },
            )),
            "helloworld"
        );
    }

    #[component]
    fn WrappedTextSoftWrapWithSeparatorApp(hooks: Hooks) -> impl Into<AnyElement<'static>> {
        let mut system = hooks.use_context_mut::<SystemContext>();
        system.exit();
        element! {
            View(width: 6) {
                Text(content: "hello world")
            }
        }
    }

    #[test]
    fn test_text_soft_wrap_selection_preserves_word_separator_when_rendered() {
        let canvases: Vec<_> = smol::block_on(
            element!(WrappedTextSoftWrapWithSeparatorApp)
                .mock_terminal_render_loop(MockTerminalConfig::default())
                .collect(),
        );
        assert_eq!(canvases.len(), 1);
        assert_eq!(canvases[0].to_string(), "hello\nworld\n");
        assert_eq!(canvases[0].soft_wrap_continuation(1), 6);
        assert_eq!(
            canvases[0].selected_text(SelectionRange::new(
                SelectionPoint { col: 0, row: 0 },
                SelectionPoint { col: 5, row: 1 },
            )),
            "hello world",
            "softWrap content-end should preserve an actual rendered word-separator space while terminal output stays trimmed"
        );
    }

    #[test]
    fn test_text_strips_ansi() {
        assert_eq!(
            element!(Text(content: "\x1b[31mhello\x1b[0m")).to_string(),
            "hello\n"
        );

        assert_eq!(
            element! {
                View(width: 10) {
                    Text(content: "\x1b[1mthis is\x1b[0m a wrap test")
                }
            }
            .to_string(),
            "this is a\nwrap test\n"
        );

        assert_eq!(
            element!(Text(content: "no ansi here")).to_string(),
            "no ansi here\n"
        );
    }

    #[test]
    fn test_text_invert() {
        let canvas = element!(Text(content: "foo", invert: true)).render(None);
        assert!(canvas.cell(0, 0).unwrap().text_style().unwrap().invert);
    }

    #[test]
    fn test_alignment_no_wrap_overflow() {
        assert_eq!(
            element! {
                View(
                    flex_direction: FlexDirection::Column,
                    width: 9,
                ) {
                    Text(
                        content: "123456789abcdef",
                        align: TextAlign::Left,
                        wrap: TextWrap::NoWrap
                    )
                }
            }
            .to_string(),
            "123456789\n"
        );

        assert_eq!(
            element! {
                View(
                    flex_direction: FlexDirection::Column,
                    width: 9,
                ) {
                    Text(
                        content: "123456789abcdef",
                        align: TextAlign::Center,
                        wrap: TextWrap::NoWrap
                    )
                }
            }
            .to_string(),
            "456789abc\n"
        );

        assert_eq!(
            element! {
                View(
                    flex_direction: FlexDirection::Column,
                    width: 9,
                ) {
                    Text(
                        content: "123456789abcdef\n1",
                        align: TextAlign::Center,
                        wrap: TextWrap::NoWrap
                    )
                }
            }
            .to_string(),
            "456789abc\n    1\n"
        );

        // If we expand the outer view, we should be able to see some of the overflowing text.
        assert_eq!(
            element! {
                View(width: 20, padding_left: 2) {
                    View(
                        flex_direction: FlexDirection::Column,
                        width: 9,
                    ) {
                        Text(
                            content: "123456789abcdef",
                            align: TextAlign::Center,
                            wrap: TextWrap::NoWrap
                        )
                    }
                }
            }
            .to_string(),
            "23456789abcdef\n"
        );

        assert_eq!(
            element! {
                View(
                    flex_direction: FlexDirection::Column,
                    width: 9,
                ) {
                    Text(
                        content: "123456789abcdef",
                        align: TextAlign::Right,
                        wrap: TextWrap::NoWrap
                    )
                }
            }
            .to_string(),
            "789abcdef\n"
        );
    }
}

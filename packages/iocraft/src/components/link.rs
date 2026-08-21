use crate::{
    component,
    components::{StyledSegment, Text},
    element, AnyElement, Props,
};

/// Props for [`Link`].
#[non_exhaustive]
#[derive(Default, Props)]
pub struct LinkProps {
    /// Hyperlink target.
    pub url: String,

    /// Text to display. Defaults to [`Self::url`].
    pub label: Option<String>,

    /// Text to display when hyperlinks are disabled. Defaults to the label/url.
    pub fallback: Option<String>,

    /// Whether to emit OSC 8 hyperlink metadata.
    ///
    /// Defaults to CC Ink-style terminal support detection. Set `Some(true)` to
    /// force OSC 8 metadata or `Some(false)` to force the fallback text.
    pub enabled: Option<bool>,
}

/// L1 (`Explicit structured Ink text-flow carrier`) for CC
/// `ink/components/Link.tsx#Link:12-31`.
///
/// Resolves Link's source-owned support/fallback branch into one structured
/// segment so nested text flows and the standalone component share it.
///
/// This is hidden from the documented API; external visibility exists only
/// for the coordinated Cometix integration across the crate boundary.
#[doc(hidden)]
pub fn link_segment(
    url: String,
    label: Option<String>,
    fallback: Option<String>,
    enabled: Option<bool>,
) -> StyledSegment {
    let label = label.unwrap_or_else(|| url.clone());
    if enabled.unwrap_or_else(crate::ansi::supports_hyperlinks) && !url.is_empty() {
        StyledSegment {
            text: label,
            hyperlink: Some(url),
            ..StyledSegment::default()
        }
    } else {
        StyledSegment::new(fallback.unwrap_or(label))
    }
}

/// Renders text with OSC 8 hyperlink metadata.
///
/// This is the iocraft counterpart to CC Ink's `<Link>` helper. It wraps
/// [`Text`] with a structured link segment when enabled so fullscreen click
/// handling and terminal hyperlink support share the same screen-buffer metadata.
#[component]
pub fn Link(props: &LinkProps) -> impl Into<AnyElement<'static>> {
    let segment = link_segment(
        props.url.clone(),
        props.label.clone(),
        props.fallback.clone(),
        props.enabled,
    );
    element!(Text(segments: Some(vec![segment])))
}

#[cfg(test)]
mod tests {
    use crate::prelude::*;

    /// Maps to CC `ink/components/Link.tsx:20-30`.
    #[test]
    fn link_segment_support_and_fallback_match_official() {
        let enabled = link_segment(
            "https://example.com".to_string(),
            Some("docs".to_string()),
            Some("plain docs".to_string()),
            Some(true),
        );
        assert_eq!(enabled.text, "docs");
        assert_eq!(enabled.hyperlink.as_deref(), Some("https://example.com"));

        let disabled = link_segment(
            "https://example.com".to_string(),
            Some("docs".to_string()),
            Some("plain docs".to_string()),
            Some(false),
        );
        assert_eq!(disabled.text, "plain docs");
        assert_eq!(disabled.hyperlink, None);
    }

    #[test]
    fn test_link_renders_osc8_hyperlink_metadata() {
        let mut link = element!(Link(
            url: "https://example.com".to_string(),
            label: Some("docs".to_string()),
            enabled: Some(true),
        ));
        let canvas = link.render(None);

        assert_eq!(canvas.to_string(), "docs\n");
        assert_eq!(
            canvas.hyperlink_at(1, 0).as_deref(),
            Some("https://example.com")
        );
    }

    #[test]
    fn test_link_fallback_disables_hyperlink_metadata() {
        let mut link = element!(Link(
            url: "https://example.com".to_string(),
            label: Some("docs".to_string()),
            fallback: Some("plain docs".to_string()),
            enabled: Some(false),
        ));
        let canvas = link.render(None);

        assert_eq!(canvas.to_string(), "plain docs\n");
        assert_eq!(canvas.hyperlink_at(1, 0), None);
    }
}

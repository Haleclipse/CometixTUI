use super::super::*;
use crate::prelude::*;
use macro_rules_attribute::apply;
use smol_macros::test;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn known(width: Option<f32>, height: Option<f32>) -> taffy::Size<Option<f32>> {
    taffy::Size { width, height }
}

fn available(width: AvailableSpace, height: AvailableSpace) -> taffy::Size<AvailableSpace> {
    taffy::Size { width, height }
}

#[test]
fn measure_key_ignores_available_height_but_not_width() {
    let definite = |value: f32| AvailableSpace::Definite(value);
    let base = MeasureKey::new(
        known(Some(80.0), None),
        available(definite(80.0), definite(100.0)),
    );
    // A flex container hands its children its own height; the key must not
    // move when it grows.
    assert_eq!(
        base,
        MeasureKey::new(
            known(Some(80.0), None),
            available(definite(80.0), definite(3800.0))
        )
    );
    assert_eq!(
        base,
        MeasureKey::new(
            known(Some(80.0), None),
            available(definite(80.0), AvailableSpace::MaxContent)
        )
    );
    // Some probes carry the container height as a known height; the measure
    // function does not read it either.
    assert_eq!(
        base,
        MeasureKey::new(
            known(Some(80.0), Some(121.0)),
            available(definite(80.0), definite(100.0))
        )
    );
    // Anything the measure functions read does move it.
    assert_ne!(
        base,
        MeasureKey::new(
            known(Some(40.0), None),
            available(definite(80.0), definite(100.0))
        )
    );
    assert_ne!(
        base,
        MeasureKey::new(
            known(Some(80.0), None),
            available(definite(40.0), definite(100.0))
        )
    );
    let min = MeasureKey::new(
        known(None, None),
        available(AvailableSpace::MinContent, definite(1.0)),
    );
    let max = MeasureKey::new(
        known(None, None),
        available(AvailableSpace::MaxContent, definite(1.0)),
    );
    let none = MeasureKey::new(known(None, None), available(definite(0.0), definite(1.0)));
    assert_ne!(min, max);
    assert_ne!(min, none);
    assert_ne!(max, none);
}

#[test]
fn measure_memo_returns_stored_sizes_and_evicts_round_robin() {
    let mut memo = MeasureMemo::default();
    let key = |width: f32| {
        MeasureKey::new(
            known(Some(width), None),
            available(AvailableSpace::Definite(width), AvailableSpace::MaxContent),
        )
    };
    let size = |height: f32| taffy::Size {
        width: 10.0,
        height,
    };
    assert_eq!(memo.get(key(1.0)), None);
    for i in 0..MEASURE_MEMO_SLOTS {
        memo.store(key(i as f32), size(i as f32));
    }
    for i in 0..MEASURE_MEMO_SLOTS {
        assert_eq!(memo.get(key(i as f32)), Some(size(i as f32)));
    }
    // One more evicts the oldest entry only.
    memo.store(key(100.0), size(100.0));
    assert_eq!(memo.get(key(0.0)), None);
    assert_eq!(memo.get(key(1.0)), Some(size(1.0)));
    assert_eq!(memo.get(key(100.0)), Some(size(100.0)));
    memo.clear();
    assert_eq!(memo.get(key(100.0)), None);
}

#[test]
fn measure_memo_reuses_the_natural_size_at_or_above_the_natural_width() {
    let mut memo = MeasureMemo::default();
    let natural = taffy::Size {
        width: 9.0,
        height: 1.0,
    };
    memo.natural = Some(natural);
    let probe = |known_width: Option<f32>, width: AvailableSpace| {
        memo.get(MeasureKey::new(
            known(known_width, None),
            available(width, AvailableSpace::Definite(3800.0)),
        ))
    };
    // Wider or equal: nothing wraps, the natural size stands.
    assert_eq!(probe(None, AvailableSpace::Definite(124.0)), Some(natural));
    assert_eq!(probe(None, AvailableSpace::Definite(9.0)), Some(natural));
    assert_eq!(
        probe(Some(110.0), AvailableSpace::Definite(110.0)),
        Some(natural)
    );
    assert_eq!(probe(Some(9.9), AvailableSpace::MaxContent), Some(natural));
    // Narrower: a real measurement is needed.
    assert_eq!(probe(None, AvailableSpace::Definite(8.0)), None);
    assert_eq!(probe(Some(8.9), AvailableSpace::MaxContent), None);
    // The min/max-content probes are never answered from the natural size:
    // max-content measures along its own path.
    assert_eq!(probe(None, AvailableSpace::MinContent), None);
    assert_eq!(probe(None, AvailableSpace::MaxContent), None);
}

// The contract `MeasureFunc` documents, checked against the crate's own text
// measure function for every wrap mode: at or above the width reported for
// a very wide definite width, a known or definite width yields that size.
#[test]
fn text_measure_func_honors_the_natural_width_contract() {
    let samples = [
        "",
        "hello world",
        "trailing spaces   ",
        "   leading spaces",
        "多字节 文字 mixed 宽度 text",
        "line one\nline two is longer   \nthird",
        "a-single-word-longer-than-most-terminals-are-wide-with-hyphens-in-it-and-no-spaces",
        "tabs\tand  double  spaces",
    ];
    let modes = [
        TextWrap::Wrap,
        TextWrap::WrapTrim,
        TextWrap::NoWrap,
        TextWrap::Truncate,
        TextWrap::TruncateEnd,
        TextWrap::TruncateMiddle,
        TextWrap::TruncateStart,
        TextWrap::End,
        TextWrap::Middle,
    ];
    let style = taffy::Style::default();
    for text in samples {
        for mode in modes {
            let f = crate::components::Text::measure_func(text.to_string(), mode);
            let natural = f(
                known(None, None),
                available(
                    AvailableSpace::Definite(NATURAL_PROBE_WIDTH),
                    AvailableSpace::MaxContent,
                ),
                &style,
            );
            for extra in [0.0, 0.9, 1.0, 37.0] {
                let width = natural.width + extra;
                let by_available = f(
                    known(None, None),
                    available(
                        AvailableSpace::Definite(width),
                        AvailableSpace::Definite(5.0),
                    ),
                    &style,
                );
                let by_known = f(
                    known(Some(width), None),
                    available(AvailableSpace::MaxContent, AvailableSpace::Definite(5.0)),
                    &style,
                );
                assert_eq!(
                    by_available, natural,
                    "{mode:?} {text:?}: available width {width} differs from the natural size"
                );
                assert_eq!(
                    by_known, natural,
                    "{mode:?} {text:?}: known width {width} differs from the natural size"
                );
            }
        }
    }
}

const ROWS: usize = 200;

// A column of wrapped text rows that grows by one row twice, then exits.
// Appending a row changes the container's height, which Taffy passes down as
// every sibling's available height; without the memo each append re-measures
// every row in the list.
#[component]
fn GrowingListApp(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let mut system = hooks.use_context_mut::<SystemContext>();
    let rows = hooks.use_state(|| ROWS);
    let mut rows_for_future = rows;
    hooks.use_future(async move {
        smol::Timer::after(Duration::from_millis(40)).await;
        rows_for_future.set(ROWS + 1);
        smol::Timer::after(Duration::from_millis(40)).await;
        rows_for_future.set(ROWS + 2);
    });
    if rows.get() == ROWS + 2 {
        system.exit();
    }
    element! {
        View(flex_direction: FlexDirection::Column) {
            #((0..rows.get()).map(|i| element! {
                Text(
                    key: i,
                    content: format!("row {i}: long enough to wrap at the mock terminal width and need a real measurement of its own"),
                )
            }))
        }
    }
}

#[apply(test!)]
async fn test_appending_a_row_does_not_remeasure_the_whole_list() {
    if crate::debug_env::disabled().measure_memo {
        // The kill-switch restores exactly the behavior this test rejects.
        return;
    }
    let measures = Arc::new(Mutex::new(Vec::new()));
    let measures_cb = Arc::clone(&measures);
    let _frames = element!(GrowingListApp)
        .mock_terminal_render_loop_with_profile(MockTerminalConfig::default(), move |event| {
            measures_cb
                .lock()
                .unwrap()
                .push(event.phases.layout_measures)
        })
        .collect::<Vec<_>>()
        .await;
    let measures = measures.lock().unwrap().clone();
    assert!(
        measures.first().is_some_and(|&first| first >= ROWS),
        "the first frame measures every row: {measures:?}"
    );
    let later = &measures[1..];
    assert!(
        later.iter().any(|&m| m > 0),
        "the appended row itself must be measured: {measures:?}"
    );
    assert!(
        later.iter().all(|&m| m < ROWS / 4),
        "appending one row re-measured the list: {measures:?}"
    );
}

// A MixedText whose contents never change next to a Text that changes every
// frame. The MixedText is updated every frame (its parent re-renders) and
// must not re-install its measure function: that would mark the node dirty
// and drop its memo, so the frame would measure it again.
#[component]
fn StableMixedTextApp(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let mut system = hooks.use_context_mut::<SystemContext>();
    let tick = hooks.use_state(|| 0u8);
    let mut tick_for_future = tick;
    hooks.use_future(async move {
        for _ in 0..3 {
            smol::Timer::after(Duration::from_millis(40)).await;
            tick_for_future.set(tick_for_future.get() + 1);
        }
    });
    if tick.get() == 3 {
        system.exit();
    }
    element! {
        View(flex_direction: FlexDirection::Column) {
            MixedText(contents: vec![
                MixedTextContent::new("stable ").color(Color::Green),
                MixedTextContent::new("mixed text that is long enough to wrap in the mock terminal"),
            ])
            Text(content: format!("tick {}", tick.get()))
        }
    }
}

#[apply(test!)]
async fn test_unchanged_mixed_text_is_not_remeasured() {
    let measures = Arc::new(Mutex::new(Vec::new()));
    let measures_cb = Arc::clone(&measures);
    let _frames = element!(StableMixedTextApp)
        .mock_terminal_render_loop_with_profile(MockTerminalConfig::default(), move |event| {
            measures_cb
                .lock()
                .unwrap()
                .push(event.phases.layout_measures)
        })
        .collect::<Vec<_>>()
        .await;
    let measures = measures.lock().unwrap().clone();
    assert!(measures.len() >= 3, "{measures:?}");
    let first = measures[0];
    // Later frames measure the changed Text only: strictly fewer probes than
    // the first frame, which measured both leaves.
    assert!(
        measures[1..].iter().all(|&m| m > 0 && m < first),
        "an unchanged MixedText was re-measured: {measures:?}"
    );
}

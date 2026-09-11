//! Demonstrates the `Checkbox` component.
//!
//! - Space/Enter toggles the focused checkbox.
//! - Tab cycles focus between the checkboxes.
//! - Children render as a space-separated label after the indicator.
//! - `checked_symbol` / `unchecked_symbol` customize the glyphs.
//!
//! Press Esc to quit.

use iocraft::prelude::*;

#[component]
fn App(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let mut system = hooks.use_context_mut::<SystemContext>();
    let mut should_exit = hooks.use_state(|| false);
    let mut focus = hooks.use_state(|| 0usize);

    let mut notifications = hooks.use_state(|| true);
    let mut telemetry = hooks.use_state(|| false);
    let mut dark_mode = hooks.use_state(|| true);

    hooks.use_terminal_events(move |event| {
        if let TerminalEvent::Key(KeyEvent {
            code,
            kind: KeyEventKind::Press,
            ..
        }) = event
        {
            match code {
                KeyCode::Esc => should_exit.set(true),
                KeyCode::Tab => focus.set((focus.get() + 1) % 3),
                KeyCode::BackTab => focus.set((focus.get() + 2) % 3),
                _ => {}
            }
        }
    });

    if should_exit.get() {
        system.exit();
    }

    element! {
        View(flex_direction: FlexDirection::Column, padding: 1) {
            Text(content: "Settings", weight: Weight::Bold, color: Color::Cyan)
            Text(content: "(Tab to move, Space/Enter to toggle, Esc to quit)", color: Color::Grey)
            View(margin_top: 1, flex_direction: FlexDirection::Column) {
                Checkbox(
                    checked: notifications.get(),
                    has_focus: focus.get() == 0,
                    on_change: move |value| notifications.set(value),
                ) {
                    Text(content: "Enable notifications")
                }
                Checkbox(
                    checked: telemetry.get(),
                    has_focus: focus.get() == 1,
                    on_change: move |value| telemetry.set(value),
                ) {
                    Text(content: "Share anonymous telemetry")
                }
                // Custom glyphs and color work alongside a label.
                Checkbox(
                    checked: dark_mode.get(),
                    has_focus: focus.get() == 2,
                    on_change: move |value| dark_mode.set(value),
                    color: Color::Green,
                    checked_symbol: "(●)".to_string(),
                    unchecked_symbol: "( )".to_string(),
                ) {
                    Text(content: "Dark mode")
                }
            }
        }
    }
}

fn main() {
    smol::block_on(element!(App).render_loop()).unwrap();
}

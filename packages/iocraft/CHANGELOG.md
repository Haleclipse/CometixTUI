# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- *(terminal)* support opt-in bracketed paste events in raw mode.
- *(hooks)* `Hooks::request_poll` schedules one `poll_change` pass over the component's hooks after the current render. Custom hooks that change what they wait for during render (a timer that starts or changes period) must call it; `use_interval` does.
- *(hooks)* `use_terminal_events_for` / `use_propagated_terminal_events_for` take a `TerminalEventInterest` (key, mouse, resize, focus, paste, response) and only receive those events; with `TerminalEventInterest::NONE` the hook holds no subscription. `TerminalEvents::set_interest` exposes the same to direct stream users. The terminal skips uninterested subscribers during dispatch — no clone, no queue entry, no wake. `View` now declares exactly what each of its five event hooks can act on, so a plain view costs nothing per keystroke; previously every view was woken and polled for every key.

### Changed

- *(render)* push-mode wake routing is now the default: each component polls its hooks through its own proxy waker and a frame's poll pass skips every component that was not woken, and render-phase state writes settle inside the frame that made them instead of committing as their own frame. `IOCRAFT_DISABLE=push-wake` restores the previous poll-everything behavior. Tests that bump a `State` during render as a frame clock, or count renders in a `State`, will see fewer frames; tick from `use_interval` / `use_future` and count in a `Ref` instead.
- *(canvas)* overlay and no-select rows are `Arc`-shared copy-on-write like cells: a new canvas shares one blank template row per buffer and copies a row only when it is first written. Building a multi-thousand-row inline canvas was allocating and zero-filling two `Vec`s per row every frame.
- *(terminal)* the blank-cell snapshot that annotates mouse events (`cell_is_blank`) is only rebuilt while the terminal is reporting the mouse. It is an O(canvas) scan per frame that serves nothing when capture is off — inline mode never enables it — and on a long inline session it was about a quarter of each frame.
- *(render)* in push mode, wakes that land during a poll pass are drained before the frame renders. Hooks are polled in order, children before parents, so an event handler that writes state polled earlier in the same pass (its own component's, or its parent's through a handler) used to leave those writes unconsumed while the pass reported Ready through a later hook; the settle pass then mistook them for render-phase writes and re-ran the whole update. The loop now re-harvests the dirty paths first, as React batches every setState of one event before rendering.
- *(render)* the push-mode poll pass now descends only into subtrees that contain a woken component: a wake marks its ancestors on the way to the render loop, and the pass consumes those marks top-down. Previously it still visited every component to read its dirty bit, which on a large tree cost more per keystroke than the update pass itself.
- *(render)* measure functions are memoized per node on the inputs they read: the known width and the available width. Taffy keys its own cache on the full known dimensions and available space, and a flex container hands its children its own height in both, so on an inline canvas every appended line changed the key of every leaf and the whole tree re-measured — over 100k text measurements and two seconds per frame on a long session. Yoga relaxes the height dimension the same way. No measure function in this crate reads either height input; the memo is cleared when the measure function or the node's style changes, and `IOCRAFT_DISABLE=measure-memo` bypasses it. `MixedText` (and structured `Text`) now re-install their measure function only when the text or wrap changed, as `Text` already did. `RenderFramePhases::layout_measures` now counts measure functions actually run. `IOCRAFT_DEBUG=layout` prints per-frame layout statistics.
- *(render)* the diagnostic environment variables are two comma-separated lists, each read once per process: `IOCRAFT_DISABLE=push-wake,retained-blit,damage-skip,measure-memo` holds the kill-switches for optimizations that are on by default, and `IOCRAFT_DEBUG=settle,cells,layout,wake,frame-log=PATH,layout-dump=PATH` holds the opt-in diagnostics (`wake` prints which component and hook slot woke each frame). Unknown tokens are ignored. The per-feature variables `IOCRAFT_PUSH_WAKE`, `IOCRAFT_RETAINED_BLIT`, `IOCRAFT_DIFF_DAMAGE_SKIP`, `IOCRAFT_SETTLE_TRACE`, `IOCRAFT_PROFILE_CHANGED_CELLS`, `IOCRAFT_FRAME_LOG` and `IOCRAFT_LAYOUT_DUMP` are no longer read, and the `IOCRAFT_FRAME_DUMP` canvas dump is removed; the frame log still reports the trailing blank row count it keyed on.

- *(hooks)* [**breaking**] `use_terminal_title` is now effect-driven like CC Ink's `useTerminalTitle`: the title is written only on the render pass where it changes, instead of on every render-loop iteration. Rewriting an unchanged title made Termux scroll back to the bottom whenever an idle app re-rendered. `SystemContext::set_terminal_title` has been removed; use the hook.
- *(terminal)* the title is delivered by the terminal backend: OSC 0 with the shared Kitty ST / BEL terminator policy on Unix-likes, and crossterm's `SetTitle` on Windows so legacy conhost without VT support still receives `SetConsoleTitleW`.
- *(hooks)* `Hook` gained `post_component_effects`, called once every hook on a component has run `post_component_update`. `use_output` drains its queue there, so output queued from `use_effect` (or hooks built on it) is written in the same update pass, matching CC Ink's synchronous `writeRaw` inside `useEffect`.

### Fixed

- *(components)* `Text` truncation no longer replaces a line that fits with an ellipsis: `truncate_line` returned `…` for any single-column width before looking at the text, so an empty or one-column line measured 1×1 at width 1 and 0×0 at every other width.
- *(hooks)* `use_terminal_title` and `use_tab_status` now write in the same update pass where their value changes. Previously the write was queued through `use_output` after that hook had already drained for the pass, so a title or tab status set on the render that also called `SystemContext::exit` was silently dropped (e.g. `examples/terminal_title.rs` never set its title).
- *(terminal)* side-band writes (`write_control_sequence`, OSC 52 clipboard, terminal queries) are flushed immediately instead of sitting in the line-buffered stdout until the next repaint. A bell, notification, or clipboard copy fired while the UI was idle previously arrived only when something else redrew.
- *(terminal)* the exit path now ends with CC Ink's `CLEAR_ITERM2_PROGRESS` (unconditional) and, when `supports_tab_status()`, the multiplexer-wrapped `CLEAR_TAB_STATUS`, so a `Working…` progress bar or tab dot no longer outlives the app.

## [0.9.1](https://github.com/ccbrown/iocraft/compare/iocraft-v0.9.0...iocraft-v0.9.1) - 2026-09-04

### Other

- fix warnings and merge conflict

## [0.9.0](https://github.com/ccbrown/iocraft/compare/iocraft-v0.8.5...iocraft-v0.9.0) - 2026-09-04

### Added

- *(iocraft)* add checkbox component ([#229](https://github.com/ccbrown/iocraft/pull/229))

### Changed

- *(terminal)* [**breaking**] add TerminalBackend trait, decouple from crossterm ([#210](https://github.com/ccbrown/iocraft/pull/210))
- `Color`, `KeyCode`, `KeyModifiers`, `KeyEventKind`, `MouseEventKind`, and `MouseButton` are now iocraft-owned types (in `crate::color`/`crate::event`) re-exported from the crate root, rather than re-exports of the crossterm types. `From` conversions to/from the crossterm equivalents are provided when the `crossterm` feature is enabled. Code that fed these directly into crossterm APIs now needs an explicit `.into()`.
- `ElementExt::write_to_raw_fd` has been renamed to `write_to_fd` and now takes `F: AsFd` instead of `F: AsRawFd`.

### Removed

- `KeyEventState` is no longer re-exported. iocraft's `KeyEvent` never carried a `state` field, so the type was unused; import it from `crossterm` directly if needed.

## [0.8.5](https://github.com/ccbrown/iocraft/compare/iocraft-v0.8.4...iocraft-v0.8.5) - 2026-08-13

### Added

- add auto_grow prop to TextInput for content-driven height ([#225](https://github.com/ccbrown/iocraft/pull/225))
- *(iocraft)* support OSC 8 hyperlinks in the canvas ([#216](https://github.com/ccbrown/iocraft/pull/216))

### Fixed

- check should_exit after select in terminal_render_loop ([#226](https://github.com/ccbrown/iocraft/pull/226))
- set max width to enable full width layout ([#223](https://github.com/ccbrown/iocraft/pull/223))
- don't consider empty rows equal to non-existent rows ([#222](https://github.com/ccbrown/iocraft/pull/222))
- *(scroll-view)* preserve auto-scroll state on no-op input ([#220](https://github.com/ccbrown/iocraft/pull/220))
- propagate terminal input errors ([#218](https://github.com/ccbrown/iocraft/pull/218))
- *(iocraft)* correct CSI final byte class and strip DCS/APC/PM sequences ([#214](https://github.com/ccbrown/iocraft/pull/214))

### Other

- Implement Hyperlinks in Text and MixedText ([#224](https://github.com/ccbrown/iocraft/pull/224))

## [0.8.4](https://github.com/ccbrown/iocraft/compare/iocraft-v0.8.3...iocraft-v0.8.4) - 2026-07-13

### Added

- *(TextInput)* add weight/underline/italic/invert style props ([#209](https://github.com/ccbrown/iocraft/pull/209))

### Fixed

- *(text_input)* preserve cursor after Unicode insertion ([#212](https://github.com/ccbrown/iocraft/pull/212))
- probe keyboard enhancement support before the first synchronized update ([#211](https://github.com/ccbrown/iocraft/pull/211))
- *(text_input)* scroll offset not recalculated when deleting in fixed width inputs ([#207](https://github.com/ccbrown/iocraft/pull/207))
- *(use_output)* corruption during async delays and stderr.print() not flushing ([#205](https://github.com/ccbrown/iocraft/pull/205))

## [0.8.3](https://github.com/ccbrown/iocraft/compare/iocraft-v0.8.2...iocraft-v0.8.3) - 2026-05-09

### Added

- add row-level diff rendering  ([#179](https://github.com/ccbrown/iocraft/pull/179))

### Fixed

- clear fullscreen terminal on resize ([#200](https://github.com/ccbrown/iocraft/pull/200))

## [0.8.2](https://github.com/ccbrown/iocraft/compare/iocraft-v0.8.1...iocraft-v0.8.2) - 2026-04-28

### Added

- add inverted text style ([#196](https://github.com/ccbrown/iocraft/pull/196))

### Fixed

- enable doc_cfg for docs generation

## [0.8.1](https://github.com/ccbrown/iocraft/compare/iocraft-v0.8.0...iocraft-v0.8.1) - 2026-04-18

### Added

- re-export taffy crate for user convenience ([#191](https://github.com/ccbrown/iocraft/pull/191))
- add public read access to cell content ([#186](https://github.com/ccbrown/iocraft/pull/186))
- strip ANSI escape codes for Text and MixedText ([#185](https://github.com/ccbrown/iocraft/pull/185))
- Add Home/End and Ctrl+A/E key bindings for TextInput ([#182](https://github.com/ccbrown/iocraft/pull/182))

### Fixed

- correctly overflow center/right+nowrap text ([#193](https://github.com/ccbrown/iocraft/pull/193))

### Other

- remove temporary type alias
- update rust, fix new clippy warnings ([#181](https://github.com/ccbrown/iocraft/pull/181))

## [0.8.0](https://github.com/ccbrown/iocraft/compare/iocraft-v0.7.18...iocraft-v0.8.0) - 2026-03-06

### Added

- allow configuring render output and stdout/stderr handles ([#157](https://github.com/ccbrown/iocraft/pull/157))
- add keyboard_scroll prop and expose auto scroll state ([#175](https://github.com/ccbrown/iocraft/pull/175))
- Add UseComponentRect ([#145](https://github.com/ccbrown/iocraft/pull/145))

### Fixed

- use i32 for use_component_rect to prevent overflow ([#176](https://github.com/ccbrown/iocraft/pull/176))

### Other

- *(deps)* bump crossterm to 0.29.0 ([#178](https://github.com/ccbrown/iocraft/pull/178))
- document use_component_rect caveat, simplify api ([#174](https://github.com/ccbrown/iocraft/pull/174))
- Scrolling component ([#170](https://github.com/ccbrown/iocraft/pull/170))

## [0.7.18](https://github.com/ccbrown/iocraft/compare/iocraft-v0.7.17...iocraft-v0.7.18) - 2026-02-17

### Added

- add disable_mouse_capture() to fullscreen render loop ([#161](https://github.com/ccbrown/iocraft/pull/161))

### Fixed

- clean visible terminal in addition to scrollback to avoid leaving behind artifacts ([#164](https://github.com/ccbrown/iocraft/pull/164))
- eliminate extra blank line in inline render mode ([#162](https://github.com/ccbrown/iocraft/pull/162))

## [0.7.17](https://github.com/ccbrown/iocraft/compare/iocraft-v0.7.16...iocraft-v0.7.17) - 2026-01-20

### Other

- eliminate any_key dependency ([#154](https://github.com/ccbrown/iocraft/pull/154))

## [0.7.16](https://github.com/ccbrown/iocraft/compare/iocraft-v0.7.15...iocraft-v0.7.16) - 2025-11-30

### Fixed

- make render_loop Send again ([#151](https://github.com/ccbrown/iocraft/pull/151))

## [0.7.15](https://github.com/ccbrown/iocraft/compare/iocraft-v0.7.14...iocraft-v0.7.15) - 2025-11-02

### Added

- allow disabling ctrl-c handling ([#149](https://github.com/ccbrown/iocraft/pull/149))
- add clonable immutable Handler ([#146](https://github.com/ccbrown/iocraft/pull/146))

### Fixed

- make fullscreen() future return type more specific

## [0.7.14](https://github.com/ccbrown/iocraft/compare/iocraft-v0.7.13...iocraft-v0.7.14) - 2025-10-08

### Fixed

- avoid bg color overflowing at eol ([#143](https://github.com/ccbrown/iocraft/pull/143))
- End synchronized update on StdTerminal drop ([#140](https://github.com/ccbrown/iocraft/pull/140))

## [0.7.13](https://github.com/ccbrown/iocraft/compare/iocraft-v0.7.12...iocraft-v0.7.13) - 2025-09-28

### Added

- use_ref, use_effect, and imperative TextInput control ([#136](https://github.com/ccbrown/iocraft/pull/136))
- additional state convenience methods

### Fixed

- underflow under certain absolute positioning circumstances ([#138](https://github.com/ccbrown/iocraft/pull/138))

## [0.7.12](https://github.com/ccbrown/iocraft/compare/iocraft-v0.7.11...iocraft-v0.7.12) - 2025-09-20

### Fixed

- purge terminal on vertical overflow ([#134](https://github.com/ccbrown/iocraft/pull/134))
- make TextInput ignore modified keys ([#132](https://github.com/ccbrown/iocraft/pull/132))

## [0.7.11](https://github.com/ccbrown/iocraft/compare/iocraft-v0.7.10...iocraft-v0.7.11) - 2025-08-20

### Added

- automatically append newline as needed for use_output ([#124](https://github.com/ccbrown/iocraft/pull/124))
- add `print` methods for stdout without newlines ([#122](https://github.com/ccbrown/iocraft/pull/122))

## [0.7.10](https://github.com/ccbrown/iocraft/compare/iocraft-v0.7.9...iocraft-v0.7.10) - 2025-06-20

### Fixed

- TextInput initial value scroll offset ([#105](https://github.com/ccbrown/iocraft/pull/105))

## [0.7.9](https://github.com/ccbrown/iocraft/compare/iocraft-v0.7.8...iocraft-v0.7.9) - 2025-05-07

### Fixed

- add gnome to list of bad vs16 terminals ([#101](https://github.com/ccbrown/iocraft/pull/101))

## [0.7.8](https://github.com/ccbrown/iocraft/compare/iocraft-v0.7.7...iocraft-v0.7.8) - 2025-04-29

### Added

- add fragment component and use_const hook ([#98](https://github.com/ccbrown/iocraft/pull/98))

## [0.7.7](https://github.com/ccbrown/iocraft/compare/iocraft-v0.7.6...iocraft-v0.7.7) - 2025-04-25

### Fixed

- don't let multiline input scroll horizontally ([#96](https://github.com/ccbrown/iocraft/pull/96))

### Other

- rewrite text input, add cursor and multiline support ([#92](https://github.com/ccbrown/iocraft/pull/92))
- implement text wrapping to be more robust for advanced cases ([#95](https://github.com/ccbrown/iocraft/pull/95))
- fix doc typo
- add UseMemo hook ([#93](https://github.com/ccbrown/iocraft/pull/93))

## [0.7.6](https://github.com/ccbrown/iocraft/compare/iocraft-v0.7.5...iocraft-v0.7.6) - 2025-04-04

### Other

- fix UseAsyncHandler docs typo

## [0.7.5](https://github.com/ccbrown/iocraft/compare/iocraft-v0.7.4...iocraft-v0.7.5) - 2025-04-03

### Fixed

- allow use_terminal_events handlers to mutate

### Other

- lint fix

## [0.7.4](https://github.com/ccbrown/iocraft/compare/iocraft-v0.7.3...iocraft-v0.7.4) - 2025-03-27

### Fixed

- don't erase last col for fullscreen tuis ([#84](https://github.com/ccbrown/iocraft/pull/84))

## [0.7.3](https://github.com/ccbrown/iocraft/compare/iocraft-v0.7.2...iocraft-v0.7.3) - 2025-03-26

### Added

- add italic text ([#82](https://github.com/ccbrown/iocraft/pull/82))

### Fixed

- don't underline leading whitespace center/right aligned text ([#81](https://github.com/ccbrown/iocraft/pull/81))

### Other

- add MixedText component ([#79](https://github.com/ccbrown/iocraft/pull/79))

## [0.7.2](https://github.com/ccbrown/iocraft/compare/iocraft-v0.7.1...iocraft-v0.7.2) - 2025-03-20

### Fixed

- don't error if keyboard enhancement check times out

## [0.7.1](https://github.com/ccbrown/iocraft/compare/iocraft-v0.7.0...iocraft-v0.7.1) - 2025-03-18

### Other

- Fix for overflow when scrolling out of bounds. ([#72](https://github.com/ccbrown/iocraft/pull/72))

## [0.7.0](https://github.com/ccbrown/iocraft/compare/iocraft-v0.6.4...iocraft-v0.7.0) - 2025-03-15

### Added

- fully implement overflow property, add scrolling example ([#70](https://github.com/ccbrown/iocraft/pull/70))

### Fixed

- negative top/left positions

### Other

- polish up docs regarding key prop ([#66](https://github.com/ccbrown/iocraft/pull/66))

## [0.6.4](https://github.com/ccbrown/iocraft/compare/iocraft-v0.6.3...iocraft-v0.6.4) - 2025-02-19

### Other

- re-arrange element macro docs to work around docs.rs bug

## [0.6.3](https://github.com/ccbrown/iocraft/compare/iocraft-v0.6.2...iocraft-v0.6.3) - 2025-02-19

### Fixed

- make properties named "key" a compile-time error

## [0.6.2](https://github.com/ccbrown/iocraft/compare/iocraft-v0.6.1...iocraft-v0.6.2) - 2025-01-20

### Fixed

- move reset to before the final newline

## [0.6.1](https://github.com/ccbrown/iocraft/compare/iocraft-v0.6.0...iocraft-v0.6.1) - 2025-01-08

### Fixed

- check if stdin is terminal (#59)

## [0.6.0](https://github.com/ccbrown/iocraft/compare/iocraft-v0.5.3...iocraft-v0.6.0) - 2024-12-30

### Added

- [**breaking**] rename `Box` to `View` to avoid conflict (#56)

## [0.5.3](https://github.com/ccbrown/iocraft/compare/iocraft-v0.5.2...iocraft-v0.5.3) - 2024-12-28

### Fixed

- eliminate undesired impact of transparent components on layout (#53)

## [0.5.2](https://github.com/ccbrown/iocraft/compare/iocraft-v0.5.1...iocraft-v0.5.2) - 2024-12-25

### Added

- add try_ methods to State, document/reduce panics (#49)

### Fixed

- improve component recycling algorithm (#51)

### Other

- add notes to State::try_ methods

## [0.5.1](https://github.com/ccbrown/iocraft/compare/iocraft-v0.5.0...iocraft-v0.5.1) - 2024-12-20

### Added

- add write function to State (#45)

### Fixed

- rename extend function to avoid std conflicts (#46)

### Other

- rust 1.83 clippy fixes (#47)
- use core instead of std where possible

## [0.5.0](https://github.com/ccbrown/iocraft/compare/iocraft-v0.4.1...iocraft-v0.5.0) - 2024-12-10

### Added

- make async functions send+sync (#38)

## [0.4.1](https://github.com/ccbrown/iocraft/compare/iocraft-v0.4.0...iocraft-v0.4.1) - 2024-12-05

### Added

- enable "std" feature for taffy ([#35](https://github.com/ccbrown/iocraft/pull/35))

## [0.4.0](https://github.com/ccbrown/iocraft/compare/iocraft-v0.3.2...iocraft-v0.4.0) - 2024-11-01

### Other

- fix minor typo

## [0.3.2](https://github.com/ccbrown/iocraft/compare/iocraft-v0.3.1...iocraft-v0.3.2) - 2024-10-04

### Added

- add button component

## [0.3.1](https://github.com/ccbrown/iocraft/compare/iocraft-v0.3.0...iocraft-v0.3.1) - 2024-09-30

### Added

- improve state api so that deadlocks are harder to create, add docs

## [0.3.0](https://github.com/ccbrown/iocraft/compare/iocraft-v0.2.3...iocraft-v0.3.0) - 2024-09-30

### Added

- convenience methods for creating terminal event types
- fullscreen mouse events, calculator example

### Other

- seal hooks, update docs, rm deprecated fn

## [0.2.3](https://github.com/ccbrown/iocraft/compare/iocraft-v0.2.2...iocraft-v0.2.3) - 2024-09-27

### Added

- add position, inset, and gap style props
- allow margins to be negative

### Other

- add test for negative margin

## [0.2.2](https://github.com/ccbrown/iocraft/compare/iocraft-v0.2.1...iocraft-v0.2.2) - 2024-09-26

### Fixed

- explicitly check for keyboard enhancement support before enabling
- make emoji with vs16 space correctly on more platforms

## [0.2.1](https://github.com/ccbrown/iocraft/compare/iocraft-v0.2.0...iocraft-v0.2.1) - 2024-09-26

### Other

- add windows to ci ([#20](https://github.com/ccbrown/iocraft/pull/20))
- use std::io::IsTerminal
- eliminate use of examples symlinks

## [0.2.0](https://github.com/ccbrown/iocraft/compare/iocraft-v0.1.2...iocraft-v0.2.0) - 2024-09-25

### Added

- add use_async_handler hook
- add mock_terminal_render_loop api

### Other

- add more docs, ratatui shoutout, and non_exhaustive attrs

## [0.1.2](https://github.com/ccbrown/iocraft/compare/iocraft-v0.1.1...iocraft-v0.1.2) - 2024-09-24

### Other

- add a few more tests
- expand documentation, add many more doc examples

## [0.1.1](https://github.com/ccbrown/iocraft/compare/iocraft-v0.1.0...iocraft-v0.1.1) - 2024-09-23

### Other

- doc improvements, add example images
- release ([#10](https://github.com/ccbrown/iocraft/pull/10))

## [0.1.0](https://github.com/ccbrown/iocraft/releases/tag/iocraft-v0.1.0) - 2024-09-23

### Fixed

- fix crate dependencies for examples
- fix doc include path resolution

### Other

- explicitly specify iocraft-macros version
- release ([#9](https://github.com/ccbrown/iocraft/pull/9))
- add package descriptions, repositories, and readmes
- key prop, docs, and tests
- documentation pass
- minor simplification
- props docs
- add docs
- add more tests
- improve test coverage
- add fullscreen example
- use_context
- refactor hook logic out of macro
- use_state
- redo hooks mechanism
- use_async refactor, spawn method
- minor refactor
- refactor terminal a bit for testability
- unicode fixes
- add form example
- rename render -> draw
- add a few more tests ([#8](https://github.com/ccbrown/iocraft/pull/8))
- add lots o tests ([#7](https://github.com/ccbrown/iocraft/pull/7))
- add ci ([#1](https://github.com/ccbrown/iocraft/pull/1))
- text underline
- text alignment
- text wrapping
- rm mouse events, avoid problematic cursor saving/restoring
- add mouse events
- typo fix
- handle emoji/different unicode character widths correctly
- rename UseFuture -> UseAsync
- tweaks to input handling
- complete first pass at docs for all public types
- do a pass at about half the docs
- clean up public api
- add license, fix up exports
- more powerful context, mutable props/context
- simplify
- simplify
- input handling and example
- eliminate render loop flickering
- system context
- small refactors, add progress bar example
- refactor stdio hooks
- polish
- cleanup
- non-static content provider props (but not value yet)
- non-static props!
- rm one more send
- less send
- simplify
- way less cloning
- context
- make handles clone
- use_stdout, use_stderr
- simplify
- tests, fix edge cases
- add tests
- convenience/Display functions, add tests
- pretty table
- text weight
- canvas rendering
- finish table example
- iterate on style support, start table example
- rename project

## [0.1.0](https://github.com/ccbrown/iocraft/releases/tag/iocraft-v0.1.0) - 2024-09-23

### Fixed

- fix crate dependencies for examples
- fix doc include path resolution

### Other

- add package descriptions, repositories, and readmes
- key prop, docs, and tests
- documentation pass
- minor simplification
- props docs
- add docs
- add more tests
- improve test coverage
- add fullscreen example
- use_context
- refactor hook logic out of macro
- use_state
- redo hooks mechanism
- use_async refactor, spawn method
- minor refactor
- refactor terminal a bit for testability
- unicode fixes
- add form example
- rename render -> draw
- add a few more tests ([#8](https://github.com/ccbrown/iocraft/pull/8))
- add lots o tests ([#7](https://github.com/ccbrown/iocraft/pull/7))
- add ci ([#1](https://github.com/ccbrown/iocraft/pull/1))
- text underline
- text alignment
- text wrapping
- rm mouse events, avoid problematic cursor saving/restoring
- add mouse events
- typo fix
- handle emoji/different unicode character widths correctly
- rename UseFuture -> UseAsync
- tweaks to input handling
- complete first pass at docs for all public types
- do a pass at about half the docs
- clean up public api
- add license, fix up exports
- more powerful context, mutable props/context
- simplify
- simplify
- input handling and example
- eliminate render loop flickering
- system context
- small refactors, add progress bar example
- refactor stdio hooks
- polish
- cleanup
- non-static content provider props (but not value yet)
- non-static props!
- rm one more send
- less send
- simplify
- way less cloning
- context
- make handles clone
- use_stdout, use_stderr
- simplify
- tests, fix edge cases
- add tests
- convenience/Display functions, add tests
- pretty table
- text weight
- canvas rendering
- finish table example
- iterate on style support, start table example
- rename project

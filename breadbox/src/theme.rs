//! The active shell theme, loaded once per process (plan §5/§6, Phase 4b-i).
//!
//! Mirrors breadbar's `src/theme.rs::shell_theme()` accessor: the underlying
//! `bread_theme::shell::load()` call happens at most once per process (via
//! the thread-local cache below), not once per call site — every place in
//! this crate that needs launcher geometry/style reads through this shared
//! instance instead of calling `bread_theme::shell::load()` itself.
//!
//! Only `ShellTheme::launcher()` is consumed today; window-spec/tokens/slots
//! accessors exist on the type but breadbox has no bar/workspace/clock
//! chrome to drive from them.

use bread_theme::shell::ShellTheme;
use std::cell::RefCell;
use std::rc::Rc;

thread_local! {
    static SHELL_THEME: RefCell<Rc<ShellTheme>> =
        RefCell::new(Rc::new(bread_theme::shell::load()));
}

/// The active shell theme. Loaded at most once per process; subsequent calls
/// just clone the cached `Rc`.
pub fn shell_theme() -> Rc<ShellTheme> {
    SHELL_THEME.with(|cell| cell.borrow().clone())
}

/// Fallback margin (pixels) for any `[launcher] top` value this function
/// can't make sense of — breadbox's pre-theme hardcoded default.
const DEFAULT_TOP_MARGIN_PX: i32 = 120;

/// Parses a `[launcher] top` value (e.g. `"120px"`) into whole pixels for
/// `Widget::set_margin_top`. Falls back to [`DEFAULT_TOP_MARGIN_PX`] — with a
/// logged reason — for any form this Phase 4b-i doesn't understand yet (a
/// bare percentage like theme 04's `"16%"`, or a malformed value in a
/// hand-edited theme) — per the manifest's "never fails to start" rule (plan
/// §4), a launcher window is not worth refusing to open over an unparseable
/// margin.
///
/// A value that parses but is negative (e.g. `"-50px"`) is clamped to `0`
/// rather than passed straight through: `Widget::set_margin_top` requires a
/// non-negative margin, and passing it a negative one silently does nothing
/// useful (a `g_return_if_fail` on the GTK side) rather than the "push the
/// panel up off-screen" a theme author might expect — 0 (flush with the top
/// edge) is the closest sane interpretation.
pub fn top_margin_px(top: &str) -> i32 {
    let trimmed = top.trim().trim_end_matches("px");
    match trimmed.parse::<i32>() {
        Ok(px) if px < 0 => {
            eprintln!(
                "breadbox: [launcher] top = {top:?} parsed to a negative margin \
                 ({px}px); clamping to 0"
            );
            0
        }
        Ok(px) => px,
        Err(_) => {
            eprintln!(
                "breadbox: [launcher] top = {top:?} is not a plain pixel value \
                 (e.g. \"120px\"); falling back to the default \
                 {DEFAULT_TOP_MARGIN_PX}px margin"
            );
            DEFAULT_TOP_MARGIN_PX
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_pixel_value() {
        assert_eq!(top_margin_px("120px"), 120);
    }

    #[test]
    fn bare_number_without_unit_suffix() {
        assert_eq!(top_margin_px("120"), 120);
    }

    #[test]
    fn percentage_form_falls_back_to_default() {
        // The doc comment's own anticipated edge case (theme 04's original
        // "16%" idea) — not parseable as a plain pixel count today.
        assert_eq!(top_margin_px("16%"), DEFAULT_TOP_MARGIN_PX);
    }

    #[test]
    fn garbage_falls_back_to_default() {
        assert_eq!(top_margin_px("not-a-length"), DEFAULT_TOP_MARGIN_PX);
    }

    #[test]
    fn negative_value_clamps_to_zero() {
        assert_eq!(top_margin_px("-50px"), 0);
    }
}

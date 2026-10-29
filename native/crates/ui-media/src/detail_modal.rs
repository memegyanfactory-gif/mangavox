//! Netflix's "More Info" modal: the page dims and a [`crate::movie_detail::MovieDetail`] (made
//! with `MovieDetail::new_modal`) opens in a centered rounded panel with a round close button on
//! its art. Escape, the close button and a click on the dimmed page close it.
use std::rc::Rc;

use gpui_kit::*;
use mv_ui::theme::radius;
use mv_ui::{ph, Tokens};

use crate::look;

/// Gap between the panel and the top / bottom of the page.
const MARGIN_Y: f32 = 32.;

/// Panel width for a page `page_width` wide: 1000px at most, with 40px clear on each side.
pub(crate) fn panel_width(page_width: f32) -> f32 {
    (page_width - 80.).min(1000.).max(320.)
}

/// The modal over a page: place it as the last child of the page's `relative()` root and focus
/// `focus` when it opens, so Escape reaches it (a mouse press on kit controls does not move
/// focus). `detail` scrolls itself; the panel only clips it. The scrim occludes the page, so cards
/// under it neither keep their hover nor take the click that closes it (see `look::overlay`).
pub(crate) fn detail_modal(focus: &FocusHandle, detail: impl Into<AnyView>, page_width: f32, on_close: impl Fn(&mut Window, &mut App) + 'static) -> Stateful<Div> {
    let on_close = Rc::new(on_close);
    let (on_esc, on_x) = (on_close.clone(), on_close.clone());
    let panel = div()
        .id("detail-modal-panel")
        .relative()
        .w(px(panel_width(page_width)))
        .h_full()
        .rounded(px(radius::MD))
        .overflow_hidden()
        .bg(Tokens::base())
        .shadow_lg()
        .occlude()
        .child(detail.into())
        .child(look::glass_button("detail-modal-close", ph::X, "Close", 36.).absolute().top(px(16.)).right(px(16.)).on_click(move |_, w, cx| on_x(w, cx)));
    div()
        .id("detail-modal")
        .absolute()
        .inset_0()
        .bg(Tokens::overlay())
        .occlude()
        .track_focus(focus)
        .on_key_down(move |ev: &KeyDownEvent, w, cx| {
            if ev.keystroke.key == "escape" {
                cx.stop_propagation();
                on_esc(w, cx);
            }
        })
        .on_mouse_down(MouseButton::Left, move |_, w, cx| on_close(w, cx))
        .child(div().absolute().left_0().right_0().top(px(MARGIN_Y)).bottom(px(MARGIN_Y)).flex().justify_center().child(look::fade_in("detail-modal-in", panel)))
}

#[cfg(test)]
mod tests {
    use super::panel_width;

    #[test]
    fn panel_keeps_clear_of_the_page_edges() {
        assert_eq!(panel_width(1920.), 1000.);
        assert_eq!(panel_width(900.), 820.);
        assert_eq!(panel_width(200.), 320.);
    }
}

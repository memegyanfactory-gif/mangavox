//! Step 6, Animate (design 9.9), always compiled: "coming next". Explains what this step will
//! do and sends the user to Cut; nothing else is clickable. W3-S5.
use gpui_kit::*;
use mv_ui::theme::space;
use mv_ui::{btn, empty_view, ph, ButtonKind};

use super::{Step, StepCtx};

const BODY: &str = "Most shots will stay as drawings, with camera moves and mouth drawings matched to the voice. Shots that need real motion will go to a video model you choose. For now, the Cut step makes an animatic from your approved frames and voices.";

pub struct AnimateStep {
    ctx: StepCtx,
}

pub fn open(ctx: StepCtx, _window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|_| AnimateStep { ctx }).into()
}

impl Render for AnimateStep {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let go = self.ctx.go.clone();
        div().size_full().flex().items_center().justify_center().p(px(space::XXL)).child(
            empty_view(ph::PLAY_CIRCLE, "Animation is coming in a later version", BODY)
                .action(btn("anime-animate-go-cut", "Go to Cut", ButtonKind::Primary).icon(Step::Cut.icon()).on_click(move |_, window, cx| go(Step::Cut, window, cx))),
        )
    }
}

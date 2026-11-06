use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{h_flex, v_flex, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{colors, list_container, mono_font};
use crate::app::GitApp;
use crate::model::DiffLineKind;

const LINE_HEIGHT: f32 = 20.0;
const LINE_NUM_WIDTH: f32 = 45.0;

impl GitApp {
    /// Diff表示（通常モード・コミットモード共通）
    pub(super) fn render_diff_view(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // ワーキングツリーの未ステージDiffのときのみHunk単位でステージできる
        let can_stage_hunk = matches!(self.current_diff_file, Some((_, false)));
        let font = mono_font(cx);

        let lines = self
            .diff
            .lines
            .iter()
            .enumerate()
            .map(|(i, line)| {
                let (bg, fg) = match line.kind {
                    DiffLineKind::Added => (Some(0x1a3a1a), 0x7ee787),
                    DiffLineKind::Removed => (Some(0x3a1a1a), 0xf85149),
                    DiffLineKind::Hunk => (Some(0x1a1a3a), 0xa371f7),
                    DiffLineKind::File => (Some(0x2a2a2a), colors::ACCENT),
                    _ => (None, colors::TEXT),
                };
                let gutter_bg = match line.kind {
                    DiffLineKind::Added | DiffLineKind::Removed => bg.unwrap_or(colors::PANEL),
                    _ => colors::PANEL,
                };
                let line_num = |n: u32| {
                    div()
                        .w(px(LINE_NUM_WIDTH))
                        .flex_none()
                        .h_full()
                        .pr_2()
                        .bg(rgb(gutter_bg))
                        .text_right()
                        .text_color(rgb(colors::DIM))
                        .child(if n > 0 { n.to_string() } else { String::new() })
                };
                let sign = match line.kind {
                    DiffLineKind::Added => "+",
                    DiffLineKind::Removed => "-",
                    _ => "",
                };
                let hunk_index = line.hunk_index;

                h_flex()
                    .id(("diff-line", i))
                    .h(px(LINE_HEIGHT))
                    .min_w_full()
                    .when_some(bg, |s, bg| s.bg(rgb(bg)))
                    .child(line_num(line.old_line_num))
                    .child(line_num(line.new_line_num))
                    .child(
                        div()
                            .w(px(24.))
                            .flex_none()
                            .text_center()
                            .text_color(rgb(fg))
                            .child(sign),
                    )
                    .child(
                        div()
                            .pl_1()
                            .pr_4()
                            .whitespace_nowrap()
                            .text_color(rgb(fg))
                            .child(line.content.clone()),
                    )
                    .when(
                        can_stage_hunk && line.kind == DiffLineKind::Hunk && hunk_index >= 0,
                        |s| {
                            s.child(
                                Button::new(("stage-hunk", i))
                                    .xsmall()
                                    .success()
                                    .label("Stage Hunk")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.stage_hunk(hunk_index as usize, cx)
                                    })),
                            )
                        },
                    )
            })
            .collect::<Vec<_>>();

        v_flex()
            .size_full()
            .gap_1()
            .child(
                h_flex()
                    .h(px(32.))
                    .flex_none()
                    .px_1()
                    .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child(
                        match &self.current_diff_file {
                            Some((name, _)) => format!("Diff — {}", name),
                            None => "Diff".to_string(),
                        },
                    ))
                    .child(
                        div()
                            .text_color(rgb(colors::MUTED))
                            .child(format!("{} lines", self.diff.total_lines)),
                    ),
            )
            .child(
                list_container().child(
                    div()
                        .id("diff-scroll")
                        .size_full()
                        .overflow_scrollbar()
                        .font_family(font)
                        .child(v_flex().py_0p5().children(lines)),
                ),
            )
    }
}

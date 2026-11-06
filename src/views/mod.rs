//! 画面描画

mod commit_panel;
mod dialogs;
mod diff;
mod graph;
mod repo_list;
mod sidebar;
mod toolbar;

use gpui_kit::component::input::{MoveDown, MoveUp};
use gpui_kit::component::{h_flex, h_resizable, resizable_panel, v_flex, v_resizable, ActiveTheme};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::GitApp;

/// 配色（旧Slint版の配色を踏襲）
pub(crate) mod colors {
    pub const BG: u32 = 0x1e1e1e;
    pub const PANEL: u32 = 0x252526;
    pub const BORDER: u32 = 0x3c3c3c;
    pub const TEXT: u32 = 0xc9d1d9;
    pub const MUTED: u32 = 0x8b949e;
    pub const DIM: u32 = 0x6e7681;
    pub const ACCENT: u32 = 0x58a6ff;
    pub const SELECTED: u32 = 0x2a2d2e;
    pub const HOVER: u32 = 0x2a2d2e;
    pub const GREEN: u32 = 0x2ec27e;
    pub const RED: u32 = 0xe01b24;
    pub const YELLOW: u32 = 0xf5c211;
    pub const PURPLE: u32 = 0x9141ac;
    pub const STATUS_BAR: u32 = 0x0d419d;
}

pub(crate) const ROW_HEIGHT: f32 = 28.0;

pub(crate) fn mono_font(cx: &App) -> SharedString {
    cx.theme().mono_font_family.clone()
}

/// ファイルステータス（A/M/D/?）のバッジ
pub(crate) fn status_badge(status: &str) -> Div {
    let color = match status {
        "A" => colors::GREEN,
        "M" => colors::YELLOW,
        "D" => colors::RED,
        _ => 0x888888,
    };
    div()
        .flex_none()
        .size(px(16.))
        .rounded(px(2.))
        .bg(rgb(color))
        .flex()
        .items_center()
        .justify_center()
        .text_xs()
        .text_color(white())
        .child(status.to_string())
}

/// セクション見出し（色付きバー + タイトル）
pub(crate) fn section_header(title: impl Into<SharedString>, accent: u32) -> Div {
    h_flex()
        .h(px(32.))
        .px_1()
        .gap_2()
        .flex_none()
        .child(div().w(px(4.)).h(px(16.)).rounded(px(2.)).bg(rgb(accent)))
        .child(
            div()
                .flex_1()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(colors::TEXT))
                .child(title.into()),
        )
}

/// 角丸の暗い背景を持つリスト領域
pub(crate) fn list_container() -> Div {
    v_flex()
        .flex_1()
        .min_h_0()
        .bg(rgb(colors::BG))
        .rounded(px(4.))
        .overflow_hidden()
}

impl Render for GitApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = if self.commit_mode {
            self.render_commit_mode(window, cx).into_any_element()
        } else {
            self.render_graph_mode(window, cx).into_any_element()
        };

        v_flex()
            .id("git-app")
            .size_full()
            .relative()
            .bg(rgb(colors::BG))
            .text_color(rgb(colors::TEXT))
            .text_sm()
            .font_family(cx.theme().font_family.clone())
            // コミットメッセージ入力欄での↑↓は履歴ナビゲーションに使う
            .capture_action(cx.listener(|this, _: &MoveUp, window, cx| {
                if this.commit_mode && this.navigate_commit_history(true, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, window, cx| {
                if this.commit_mode && this.navigate_commit_history(false, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(self.render_toolbar(window, cx))
            .when(!self.status_message.is_empty(), |this| {
                this.child(
                    div()
                        .h(px(20.))
                        .flex_none()
                        .bg(rgb(colors::STATUS_BAR))
                        .text_xs()
                        .text_color(white())
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(self.status_message.clone()),
                )
            })
            .child(
                div().flex_1().min_h_0().child(
                    h_resizable("main-split")
                        .child(
                            resizable_panel()
                                .size(px(220.))
                                .size_range(px(140.)..px(420.))
                                .child(self.render_sidebar(window, cx)),
                        )
                        .child(resizable_panel().child(content)),
                ),
            )
            .when(self.show_repo_sidebar, |this| {
                this.child(self.render_repo_sidebar(window, cx))
            })
    }
}

impl GitApp {
    /// 通常モード: コミットグラフ + 変更ファイル/Diff
    fn render_graph_mode(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        v_resizable("graph-split")
            .child(resizable_panel().child(self.render_commit_graph(window, cx)))
            .child(
                resizable_panel()
                    .size(px(300.))
                    .size_range(px(100.)..px(2000.))
                    .child(self.render_commit_details(window, cx)),
            )
    }

    /// コミットモード: Staged/Unstaged/コミット入力 + Diff
    fn render_commit_mode(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_resizable("commit-split")
            .child(
                resizable_panel()
                    .size(px(560.))
                    .size_range(px(300.)..px(1200.))
                    .child(self.render_commit_panel(window, cx)),
            )
            .child(resizable_panel().child(self.render_diff_view(window, cx)))
    }
}

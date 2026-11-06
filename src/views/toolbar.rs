use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{h_flex, Disableable, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::colors;
use crate::app::GitApp;

impl GitApp {
    pub(super) fn render_toolbar(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let has_repo = self.repo_path.is_some();
        let busy = self.is_remote_busy();

        h_flex()
            .h(px(42.))
            .flex_none()
            .px_2()
            .gap_2()
            .bg(rgb(colors::PANEL))
            .border_b_1()
            .border_color(rgb(colors::BORDER))
            .child(
                Button::new("toggle-repo-list")
                    .ghost()
                    .label(if self.show_repo_sidebar {
                        "📂"
                    } else {
                        "📁"
                    })
                    .tooltip("Repositories")
                    .on_click(cx.listener(|this, _, _, cx| {
                        if !this.show_repo_sidebar {
                            this.set_repo_entries(crate::config::load_repo_entries());
                        }
                        this.show_repo_sidebar = !this.show_repo_sidebar;
                        cx.notify();
                    })),
            )
            .child(div().font_weight(FontWeight::SEMIBOLD).text_base().child(
                if self.repo_name.is_empty() {
                    "Select Repository".to_string()
                } else {
                    self.repo_name.clone()
                },
            ))
            .when(!self.snapshot.current_branch.is_empty(), |this| {
                this.child(
                    div()
                        .text_color(rgb(colors::GREEN))
                        .child(format!("⎇ {}", self.snapshot.current_branch)),
                )
            })
            .child(div().w(px(8.)))
            .child(
                Button::new("pull")
                    .small()
                    .label("⬇ Pull")
                    .disabled(!has_repo || busy)
                    .on_click(cx.listener(|this, _, _, cx| this.pull(cx))),
            )
            .child(
                Button::new("push")
                    .small()
                    .label("⬆ Push")
                    .disabled(!has_repo || busy)
                    .on_click(cx.listener(|this, _, _, cx| this.push(cx))),
            )
            .child(
                Button::new("fetch")
                    .small()
                    .label("⟳ Refresh & Fetch")
                    .disabled(!has_repo || busy)
                    .loading(busy)
                    .on_click(cx.listener(|this, _, _, cx| this.fetch_and_refresh(cx))),
            )
            .child(div().flex_1())
            .when(self.loading, |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(rgb(colors::MUTED))
                        .child("Loading..."),
                )
            })
    }
}

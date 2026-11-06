use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::Textarea;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{h_flex, v_flex, Disableable, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{colors, dialogs, list_container, status_badge, ROW_HEIGHT};
use crate::app::{FileSelection, GitApp};

impl GitApp {
    pub(super) fn render_commit_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let staged_len = self.snapshot.staged.len();
        let unstaged_len = self.snapshot.unstaged.len();
        let staged_checked = self.staged_check.count();
        let unstaged_checked = self.unstaged_check.count();
        let can_commit = self.can_commit(cx);

        let list_header = |title: String, color: u32, checked: usize| {
            h_flex()
                .h(px(36.))
                .flex_none()
                .px_1()
                .gap_1()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(color))
                        .child(title),
                )
                .when(checked > 0, |s| {
                    s.child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors::MUTED))
                            .child(format!("• {} selected", checked)),
                    )
                })
                .child(div().flex_1())
        };

        v_flex()
            .size_full()
            .p_1()
            .gap_1()
            .bg(rgb(colors::PANEL))
            // Staged
            .child(
                list_header(
                    format!("Staged ({})", staged_len),
                    colors::GREEN,
                    staged_checked,
                )
                .child(
                    Button::new("stash")
                        .small()
                        .label("Stash")
                        .on_click(cx.listener(|_, _, window, cx| {
                            dialogs::open_create_stash_dialog(cx.entity().downgrade(), window, cx)
                        })),
                )
                .child(
                    Button::new("unstage-selected")
                        .small()
                        .label("Unstage Selected")
                        .disabled(staged_checked == 0)
                        .on_click(cx.listener(|this, _, _, cx| this.unstage_selected(cx))),
                )
                .child(
                    Button::new("unstage-all")
                        .small()
                        .label("Unstage All")
                        .disabled(staged_len == 0)
                        .on_click(cx.listener(|this, _, _, cx| this.unstage_all(cx))),
                ),
            )
            .child(self.render_file_list(true, window, cx))
            // Unstaged
            .child(
                list_header(
                    format!("Unstaged ({})", unstaged_len),
                    colors::RED,
                    unstaged_checked,
                )
                .child(
                    Button::new("stage-selected")
                        .small()
                        .label("Stage Selected")
                        .disabled(unstaged_checked == 0)
                        .on_click(cx.listener(|this, _, _, cx| this.stage_selected(cx))),
                )
                .child(
                    Button::new("stage-all")
                        .small()
                        .label("Stage All")
                        .disabled(unstaged_len == 0)
                        .on_click(cx.listener(|this, _, _, cx| this.stage_all(cx))),
                )
                .child(div().w(px(12.)))
                .child(
                    Button::new("discard-selected")
                        .small()
                        .danger()
                        .label("Discard")
                        .disabled(unstaged_checked == 0)
                        .on_click(cx.listener(|_, _, window, cx| {
                            dialogs::confirm_discard(cx.entity().downgrade(), window, cx)
                        })),
                ),
            )
            .child(self.render_file_list(false, window, cx))
            // Commit Message
            .child(
                h_flex()
                    .h(px(32.))
                    .flex_none()
                    .px_1()
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Commit Message"),
                    )
                    .child(
                        Button::new("commit-history")
                            .small()
                            .label("History")
                            .disabled(self.commit_history.is_empty())
                            .on_click(cx.listener(|_, _, window, cx| {
                                dialogs::open_commit_history_dialog(
                                    cx.entity().downgrade(),
                                    window,
                                    cx,
                                )
                            })),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .h(px(96.))
                    .child(Textarea::new(&self.commit_input).h_full()),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .py_1()
                    .child(
                        Button::new("commit")
                            .primary()
                            .label("Commit")
                            .disabled(!can_commit)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.commit(false, window, cx)),
                            ),
                    )
                    .child(
                        Button::new("commit-push")
                            .label("Commit & Push ⬆")
                            .disabled(!can_commit)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.commit(true, window, cx)),
                            ),
                    ),
            )
    }

    fn render_file_list(
        &mut self,
        staged: bool,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let checks = if staged {
            &self.staged_check
        } else {
            &self.unstaged_check
        };
        let rows = self
            .files(staged)
            .iter()
            .enumerate()
            .map(|(idx, file)| {
                let selected = self.selected_file == Some(FileSelection { staged, index: idx });
                let checked = checks.is_checked(idx);
                let filename = file.filename.clone();
                let id_base = if staged { 0 } else { 1_000_000 };

                h_flex()
                    .id(("file", id_base + idx))
                    .h(px(ROW_HEIGHT))
                    .px_1()
                    .gap_1()
                    .cursor_pointer()
                    .when(selected, |s| s.bg(rgb(colors::SELECTED)))
                    .when(!selected && checked, |s| s.bg(rgb(0x1a2a3a)))
                    .hover(|s| s.bg(rgb(colors::HOVER)))
                    .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                        let modifiers = event.modifiers();
                        if modifiers.control || modifiers.platform {
                            this.toggle_check(staged, idx, cx);
                        } else if modifiers.shift {
                            this.range_select(staged, idx, cx);
                        } else {
                            this.select_file(staged, idx, cx);
                        }
                    }))
                    .child(
                        Checkbox::new(("file-check", id_base + idx))
                            .checked(checked)
                            .on_click(cx.listener(move |this, _: &bool, _, cx| {
                                this.toggle_check(staged, idx, cx)
                            })),
                    )
                    .child(status_badge(&file.status))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(rgb(if selected || checked {
                                colors::ACCENT
                            } else {
                                colors::TEXT
                            }))
                            .child(file.filename.clone()),
                    )
                    .child(
                        Button::new(("file-stage", id_base + idx))
                            .xsmall()
                            .label(if staged { "−" } else { "+" })
                            .tooltip(if staged { "Unstage" } else { "Stage" })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                if staged {
                                    this.unstage_file(&filename, cx);
                                } else {
                                    this.stage_file(&filename, cx);
                                }
                            })),
                    )
            })
            .collect::<Vec<_>>();

        list_container().child(
            div()
                .id(if staged {
                    "staged-list"
                } else {
                    "unstaged-list"
                })
                .size_full()
                .overflow_y_scrollbar()
                .child(v_flex().children(rows)),
        )
    }
}

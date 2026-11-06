use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{h_flex, resizable_panel, v_flex, v_resizable, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{colors, dialogs, list_container, section_header, ROW_HEIGHT};
use crate::app::GitApp;

/// ブランチのコンテキストメニュー項目を追加する
///
/// `from_sidebar` はサイドバー（ローカルブランチ一覧）から開いた場合。
/// その場合のみPR作成・マージ・削除を表示する。
pub(super) fn branch_menu(
    menu: PopupMenu,
    app: WeakEntity<GitApp>,
    name: String,
    is_remote: bool,
    is_current: bool,
    from_sidebar: bool,
) -> PopupMenu {
    let item = |label: &str, action: fn(&mut GitApp, &str, &mut Context<GitApp>)| {
        let app = app.clone();
        let name = name.clone();
        PopupMenuItem::new(label.to_string()).on_click(move |_, _, cx| {
            app.update(cx, |this, cx| action(this, &name, cx)).ok();
        })
    };

    let mut menu = menu
        .item(item(
            "Checkout Branch",
            if is_remote {
                GitApp::checkout_remote_branch
            } else {
                GitApp::checkout_branch
            },
        ))
        .item(item("Copy Branch Name", GitApp::copy_text));

    if from_sidebar {
        menu = menu.item(item("Create Pull Request", GitApp::create_pull_request));
        if !is_current && !is_remote {
            menu = menu
                .separator()
                .item(item("Merge into Current", GitApp::merge_branch))
                .item(item("Delete Branch", GitApp::delete_branch));
        }
    }
    menu
}

impl GitApp {
    pub(super) fn render_sidebar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mode_button = if self.commit_mode {
            Button::new("commit-mode")
                .label("← Back")
                .w_full()
                .on_click(cx.listener(|this, _, _, cx| this.exit_commit_mode(cx)))
        } else {
            Button::new("commit-mode")
                .primary()
                .label("📝 Commit")
                .w_full()
                .on_click(cx.listener(|this, _, _, cx| this.enter_commit_mode(cx)))
        };

        v_flex()
            .size_full()
            .bg(rgb(colors::PANEL))
            .child(div().p_1().flex_none().child(mode_button))
            .child(
                div().flex_1().min_h_0().child(
                    v_resizable("sidebar-split")
                        .child(
                            resizable_panel()
                                .size(px(240.))
                                .size_range(px(80.)..px(800.))
                                .child(self.render_local_branches(window, cx)),
                        )
                        .child(
                            resizable_panel()
                                .size(px(200.))
                                .size_range(px(60.)..px(800.))
                                .child(self.render_remote_branches(window, cx)),
                        )
                        .child(
                            resizable_panel()
                                .size_range(px(60.)..px(800.))
                                .child(self.render_stashes(window, cx)),
                        ),
                ),
            )
    }

    fn render_local_branches(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let app = cx.entity().downgrade();
        let rows = self
            .snapshot
            .local_branches
            .iter()
            .enumerate()
            .map(|(idx, branch)| {
                let selected = self.selected_branch == Some(idx);
                let name = branch.name.clone();
                let is_current = branch.is_current;
                let menu_app = app.clone();
                let menu_name = name.clone();

                h_flex()
                    .id(("local-branch", idx))
                    .h(px(ROW_HEIGHT))
                    .px_2()
                    .gap_1()
                    .cursor_pointer()
                    .when(selected, |s| s.bg(rgb(colors::SELECTED)))
                    .when(!selected && is_current, |s| s.bg(rgb(0x1a3a1a)))
                    .hover(|s| s.bg(rgb(colors::HOVER)))
                    .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                        this.selected_branch = Some(idx);
                        if event.click_count() >= 2 && !is_current {
                            this.checkout_branch(&name, cx);
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(14.))
                            .text_color(rgb(if is_current { colors::GREEN } else { 0x555555 }))
                            .child(if is_current { "●" } else { "○" }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .when(is_current, |s| s.font_weight(FontWeight::SEMIBOLD))
                            .text_color(rgb(if selected {
                                colors::ACCENT
                            } else if is_current {
                                colors::GREEN
                            } else {
                                colors::TEXT
                            }))
                            .child(branch.name.clone()),
                    )
                    .when(branch.ahead > 0 || branch.behind > 0, |s| {
                        s.child(
                            h_flex()
                                .flex_none()
                                .gap_1()
                                .px_1p5()
                                .rounded(px(4.))
                                .bg(rgb(colors::BORDER))
                                .text_xs()
                                .when(branch.ahead > 0, |s| {
                                    s.child(
                                        div()
                                            .text_color(rgb(0x7ee787))
                                            .child(format!("↑{}", branch.ahead)),
                                    )
                                })
                                .when(branch.behind > 0, |s| {
                                    s.child(
                                        div()
                                            .text_color(rgb(0xf97583))
                                            .child(format!("↓{}", branch.behind)),
                                    )
                                }),
                        )
                    })
                    .context_menu(move |menu, _, _| {
                        branch_menu(
                            menu,
                            menu_app.clone(),
                            menu_name.clone(),
                            false,
                            is_current,
                            true,
                        )
                    })
            })
            .collect::<Vec<_>>();

        let count = rows.len();
        v_flex()
            .size_full()
            .p_1()
            .gap_1()
            .child(
                section_header(format!("Local ({})", count), colors::GREEN).child(
                    Button::new("new-branch")
                        .xsmall()
                        .ghost()
                        .label("+")
                        .tooltip("Create Branch")
                        .on_click(cx.listener(|_, _, window, cx| {
                            dialogs::open_create_branch_dialog(
                                cx.entity().downgrade(),
                                None,
                                window,
                                cx,
                            )
                        })),
                ),
            )
            .child(
                list_container().child(
                    div()
                        .id("local-branch-list")
                        .size_full()
                        .overflow_y_scrollbar()
                        .child(v_flex().children(rows)),
                ),
            )
    }

    fn render_remote_branches(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let app = cx.entity().downgrade();
        let rows = self
            .snapshot
            .remote_branches
            .iter()
            .enumerate()
            .map(|(idx, branch)| {
                let selected = self.selected_remote_branch == Some(idx);
                let name = branch.name.clone();
                let menu_app = app.clone();
                let menu_name = name.clone();

                h_flex()
                    .id(("remote-branch", idx))
                    .h(px(ROW_HEIGHT))
                    .px_2()
                    .gap_1()
                    .cursor_pointer()
                    .when(selected, |s| s.bg(rgb(colors::SELECTED)))
                    .hover(|s| s.bg(rgb(colors::HOVER)))
                    .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                        this.selected_remote_branch = Some(idx);
                        if event.click_count() >= 2 {
                            this.checkout_remote_branch(&name, cx);
                        }
                        cx.notify();
                    }))
                    .child(div().w(px(14.)).text_color(rgb(0x666666)).child("↓"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(rgb(if selected {
                                colors::ACCENT
                            } else {
                                colors::MUTED
                            }))
                            .child(branch.name.clone()),
                    )
                    .context_menu(move |menu, _, _| {
                        branch_menu(
                            menu,
                            menu_app.clone(),
                            menu_name.clone(),
                            true,
                            false,
                            false,
                        )
                    })
            })
            .collect::<Vec<_>>();

        let count = rows.len();
        v_flex()
            .size_full()
            .p_1()
            .gap_1()
            .child(section_header(format!("Remote ({})", count), 0x666666))
            .child(
                list_container().child(
                    div()
                        .id("remote-branch-list")
                        .size_full()
                        .overflow_y_scrollbar()
                        .child(v_flex().children(rows)),
                ),
            )
    }

    fn render_stashes(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let app = cx.entity().downgrade();
        let rows = self
            .snapshot
            .stashes
            .iter()
            .map(|stash| {
                let index = stash.index;
                let menu_app = app.clone();

                h_flex()
                    .id(("stash", index))
                    .h(px(ROW_HEIGHT))
                    .px_2()
                    .gap_1()
                    .hover(|s| s.bg(rgb(colors::HOVER)))
                    .child(div().w(px(18.)).child("📦"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(rgb(colors::MUTED))
                            .child(format!("{}: {}", index, stash.message)),
                    )
                    .context_menu(move |menu, _, _| {
                        let item =
                            |label: &str, action: fn(&mut GitApp, usize, &mut Context<GitApp>)| {
                                let app = menu_app.clone();
                                PopupMenuItem::new(label.to_string()).on_click(move |_, _, cx| {
                                    app.update(cx, |this, cx| action(this, index, cx)).ok();
                                })
                            };
                        menu.item(item("Apply Stash", GitApp::stash_apply))
                            .item(item("Pop Stash", GitApp::stash_pop))
                            .separator()
                            .item(item("Drop Stash", GitApp::stash_drop))
                    })
            })
            .collect::<Vec<_>>();

        let count = rows.len();
        v_flex()
            .size_full()
            .p_1()
            .gap_1()
            .child(
                section_header(format!("Stashes ({})", count), colors::PURPLE).child(
                    Button::new("new-stash")
                        .xsmall()
                        .ghost()
                        .label("+")
                        .tooltip("Create Stash")
                        .on_click(cx.listener(|_, _, window, cx| {
                            dialogs::open_create_stash_dialog(cx.entity().downgrade(), window, cx)
                        })),
                ),
            )
            .child(
                list_container().child(
                    div()
                        .id("stash-list")
                        .size_full()
                        .overflow_y_scrollbar()
                        .child(v_flex().children(rows)),
                ),
            )
    }
}

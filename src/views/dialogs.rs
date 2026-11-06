//! ダイアログ（入力・確認）

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{h_flex, v_flex, Disableable, WindowExt};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::colors;
use crate::app::{short_hash, GitApp};

type Submit = Rc<dyn Fn(String, &mut Window, &mut App)>;

/// 1行入力のダイアログを開く
fn open_input_dialog(
    title: &'static str,
    description: Option<String>,
    label: &'static str,
    placeholder: &'static str,
    ok_label: &'static str,
    allow_empty: bool,
    window: &mut Window,
    cx: &mut App,
    on_submit: impl Fn(String, &mut Window, &mut App) + 'static,
) {
    let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
    let on_submit: Submit = Rc::new(on_submit);

    let submit = {
        let input = input.clone();
        move |window: &mut Window, cx: &mut App| {
            let value = input.read(cx).value().trim().to_string();
            if value.is_empty() && !allow_empty {
                return;
            }
            window.close_dialog(cx);
            on_submit(value, window, cx);
        }
    };
    let submit = Rc::new(submit);

    // Enterで確定
    {
        let submit = submit.clone();
        window
            .subscribe(&input, cx, move |_, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    submit(window, cx);
                }
            })
            .detach();
    }

    let initial_focus = Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if initial_focus.replace(false) {
            input.focus_handle(cx).focus(window, cx);
        }
        let submit = submit.clone();
        dialog
            .title(title)
            .w(px(440.))
            .child(
                v_flex()
                    .gap_2()
                    .when_some(description.clone(), |s, d| {
                        s.child(div().text_sm().text_color(rgb(colors::MUTED)).child(d))
                    })
                    .child(div().text_sm().child(label))
                    .child(Input::new(&input)),
            )
            .footer(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("cancel")
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("ok")
                            .primary()
                            .label(ok_label)
                            .on_click(move |_, window, cx| submit(window, cx)),
                    ),
            )
    });
}

/// 確認ダイアログを開く
fn confirm(
    title: &'static str,
    description: String,
    ok_label: &'static str,
    danger: bool,
    window: &mut Window,
    cx: &mut App,
    on_ok: impl Fn(&mut Window, &mut App) + 'static,
) {
    let on_ok = Rc::new(on_ok);
    window.open_alert_dialog(cx, move |alert, _, _| {
        let on_ok = on_ok.clone();
        alert
            .confirm()
            .title(title)
            .description(description.clone())
            .ok_text(ok_label)
            .ok_variant(if danger {
                ButtonVariant::Danger
            } else {
                ButtonVariant::Primary
            })
            .on_ok(move |_, window, cx| {
                on_ok(window, cx);
                true
            })
    });
}

pub(crate) fn open_create_branch_dialog(
    app: WeakEntity<GitApp>,
    at_commit: Option<String>,
    window: &mut Window,
    cx: &mut App,
) {
    let (title, description) = match &at_commit {
        Some(hash) => (
            "Create Branch at Commit",
            Some(format!("Commit: {}", short_hash(hash))),
        ),
        None => ("Create Branch", None),
    };
    open_input_dialog(
        title,
        description,
        "Branch Name",
        "new-feature-branch",
        "Create",
        false,
        window,
        cx,
        move |name, _, cx| {
            app.update(cx, |this, cx| {
                this.create_branch(&name, at_commit.as_deref(), cx)
            })
            .ok();
        },
    );
}

pub(crate) fn open_create_stash_dialog(app: WeakEntity<GitApp>, window: &mut Window, cx: &mut App) {
    open_input_dialog(
        "Create Stash",
        None,
        "Message (optional)",
        "WIP: description...",
        "Stash",
        true,
        window,
        cx,
        move |message, _, cx| {
            app.update(cx, |this, cx| this.stash_save(&message, cx))
                .ok();
        },
    );
}

pub(crate) fn open_create_group_dialog(app: WeakEntity<GitApp>, window: &mut Window, cx: &mut App) {
    open_input_dialog(
        "Create Group",
        None,
        "Group Name",
        "Group name",
        "Create",
        false,
        window,
        cx,
        move |name, _, cx| {
            app.update(cx, |this, cx| {
                this.update_repo_entries(crate::config::create_repo_group(&name), cx)
            })
            .ok();
        },
    );
}

pub(crate) fn confirm_discard(app: WeakEntity<GitApp>, window: &mut Window, cx: &mut App) {
    let count = app
        .read_with(cx, |this, _| this.unstaged_check.count())
        .unwrap_or(0);
    confirm(
        "Discard Changes",
        format!(
            "Discard changes in {} selected file(s)? This cannot be undone.",
            count
        ),
        "Discard",
        true,
        window,
        cx,
        move |_, cx| {
            app.update(cx, |this, cx| this.discard_selected(cx)).ok();
        },
    );
}

pub(crate) fn confirm_hard_reset(
    app: WeakEntity<GitApp>,
    hash: String,
    window: &mut Window,
    cx: &mut App,
) {
    confirm(
        "Hard Reset",
        format!(
            "Reset the current branch to {} and discard all uncommitted changes?",
            short_hash(&hash)
        ),
        "Reset",
        true,
        window,
        cx,
        move |_, cx| {
            app.update(cx, |this, cx| this.reset_to_commit(&hash, "hard", cx))
                .ok();
        },
    );
}

pub(crate) fn confirm_remove_repo(
    app: WeakEntity<GitApp>,
    item_index: usize,
    path: String,
    window: &mut Window,
    cx: &mut App,
) {
    confirm(
        "Remove Repository",
        format!(
            "Remove from the list?\n{}\n(Files on disk are not deleted.)",
            path
        ),
        "Remove",
        true,
        window,
        cx,
        move |_, cx| {
            app.update(cx, |this, cx| {
                this.update_repo_entries(crate::config::remove_repo_item(item_index), cx)
            })
            .ok();
        },
    );
}

pub(crate) fn confirm_delete_group(
    app: WeakEntity<GitApp>,
    group_index: usize,
    name: String,
    repo_count: i32,
    window: &mut Window,
    cx: &mut App,
) {
    confirm(
        "Delete Group",
        format!(
            "Delete group \"{}\" and remove its {} repositories from the list?",
            name, repo_count
        ),
        "Delete",
        true,
        window,
        cx,
        move |_, cx| {
            app.update(cx, |this, cx| {
                this.update_repo_entries(crate::config::delete_repo_group(group_index), cx)
            })
            .ok();
        },
    );
}

pub(crate) fn open_commit_history_dialog(
    app: WeakEntity<GitApp>,
    window: &mut Window,
    cx: &mut App,
) {
    window.open_dialog(cx, move |dialog, _, cx| {
        let history = app
            .read_with(cx, |this, _| this.commit_history.clone())
            .unwrap_or_default();
        let rows = history.into_iter().enumerate().map(|(i, message)| {
            let app = app.clone();
            div()
                .id(("history", i))
                .px_2()
                .py_1p5()
                .rounded(px(4.))
                .cursor_pointer()
                .hover(|s| s.bg(rgb(colors::HOVER)))
                .text_sm()
                .child(message)
                .on_click(move |_, window, cx| {
                    window.close_dialog(cx);
                    app.update(cx, |this, cx| this.use_commit_history(i, window, cx))
                        .ok();
                })
        });
        dialog.title("Commit Message History").w(px(560.)).child(
            div()
                .id("history-list")
                .max_h(px(400.))
                .overflow_y_scrollbar()
                .child(v_flex().gap_0p5().children(rows)),
        )
    });
}

/// クローンダイアログの状態
struct CloneForm {
    app: Entity<GitApp>,
    url: Entity<InputState>,
    path: Entity<InputState>,
    error: Option<String>,
    cloning: bool,
}

impl CloneForm {
    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = self.url.read(cx).value().trim().to_string();
        let path = self.path.read(cx).value().trim().to_string();
        if url.is_empty() || path.is_empty() || self.cloning {
            return;
        }
        self.cloning = true;
        self.error = None;
        cx.notify();

        let form = cx.entity().downgrade();
        self.app.update(cx, |app, cx| {
            app.clone_repo(
                url,
                path,
                move |result, window, cx| match result {
                    Ok(()) => window.close_dialog(cx),
                    Err(e) => {
                        form.update(cx, |form, cx| {
                            form.cloning = false;
                            form.error = Some(e);
                            cx.notify();
                        })
                        .ok();
                    }
                },
                window,
                cx,
            )
        });
    }
}

impl Render for CloneForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let path_input = self.path.clone();
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(colors::MUTED))
                    .child("Repository URL"),
            )
            .child(Input::new(&self.url).disabled(self.cloning))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(colors::MUTED))
                    .child("Destination Path"),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&self.path).disabled(self.cloning)),
                    )
                    .child(
                        Button::new("browse-clone-path")
                            .label("Browse...")
                            .disabled(self.cloning)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let input = path_input.clone();
                                this.app
                                    .update(cx, |app, cx| app.browse_clone_path(input, cx));
                            })),
                    ),
            )
            .when_some(self.error.clone(), |s, e| {
                s.child(div().text_sm().text_color(rgb(0xf85149)).child(e))
            })
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .pt_2()
                    .child(
                        Button::new("cancel-clone")
                            .label("Cancel")
                            .disabled(self.cloning)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("start-clone")
                            .primary()
                            .label(if self.cloning { "Cloning..." } else { "Clone" })
                            .loading(self.cloning)
                            .on_click(cx.listener(|this, _, window, cx| this.start(window, cx))),
                    ),
            )
    }
}

pub(crate) fn open_clone_dialog(app: Entity<GitApp>, window: &mut Window, cx: &mut App) {
    let url = cx
        .new(|cx| InputState::new(window, cx).placeholder("https://github.com/username/repo.git"));
    let path = cx.new(|cx| InputState::new(window, cx).placeholder("/path/to/destination"));
    let form = cx.new(|_| CloneForm {
        app,
        url: url.clone(),
        path,
        error: None,
        cloning: false,
    });

    let initial_focus = Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if initial_focus.replace(false) {
            url.focus_handle(cx).focus(window, cx);
        }
        dialog
            .title("Clone Repository")
            .w(px(560.))
            .child(form.clone())
    });
}

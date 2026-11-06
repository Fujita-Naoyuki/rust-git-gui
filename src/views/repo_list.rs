use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{h_flex, v_flex, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{colors, dialogs, list_container};
use crate::app::{DraggedRepo, DropZone, GitApp};

const ITEM_HEIGHT: f32 = 32.0;
const SIDEBAR_WIDTH: f32 = 600.0;

/// ドラッグ中に表示するプレビュー
struct DragPreview {
    label: String,
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded(px(4.))
            .bg(rgb(0x3584e4))
            .text_color(white())
            .text_sm()
            .child(self.label.clone())
    }
}

impl GitApp {
    pub(super) fn render_repo_sidebar(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let current_path = self.repo_path.clone().unwrap_or_default();
        let item_count = self.repo_items.len();

        let rows = self
            .repo_items
            .iter()
            .enumerate()
            .map(|(idx, item)| {
                let drop_zone = self
                    .drag_target
                    .filter(|(target, _)| *target == idx)
                    .map(|(_, zone)| zone);
                let is_current = !item.is_group && item.repo_path == current_path;
                let item = item.clone();

                let mut row = h_flex()
                    .id(("repo-item", idx))
                    .relative()
                    .h(px(ITEM_HEIGHT))
                    .px_2()
                    .gap_1()
                    .cursor_pointer()
                    .when(item.is_group, |s| s.bg(rgb(colors::PANEL)))
                    .when(is_current, |s| s.bg(rgb(0x3584e4)))
                    .when(
                        item.is_group
                            && matches!(drop_zone, Some(DropZone::Center | DropZone::After)),
                        |s| s.bg(rgba(0x3584e440)),
                    )
                    .when(!is_current, |s| s.hover(|s| s.bg(rgb(colors::HOVER))))
                    // ドロップ位置の判定（行の上端/中央/下端）
                    .on_drag_move(cx.listener(
                        move |this, event: &DragMoveEvent<DraggedRepo>, _, cx| {
                            let bounds = event.bounds;
                            let pos = event.event.position;
                            if !bounds.contains(&pos) {
                                return;
                            }
                            let offset = pos.y - bounds.top();
                            let zone = if offset < px(8.) {
                                DropZone::Before
                            } else if offset > bounds.size.height - px(8.) {
                                DropZone::After
                            } else {
                                DropZone::Center
                            };
                            if this.drag_target != Some((idx, zone)) {
                                this.drag_target = Some((idx, zone));
                                cx.notify();
                            }
                        },
                    ))
                    .on_drop(cx.listener(move |this, dragged: &DraggedRepo, _, cx| {
                        this.drop_repo(dragged.item_index, idx, cx);
                    }));

                // ドロップ位置インジケーター
                let indicator = |top: bool| {
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .h(px(2.))
                        .bg(rgb(0x3584e4))
                        .when(top, |s| s.top_0())
                        .when(!top, |s| s.bottom_0())
                };
                if drop_zone == Some(DropZone::Before) {
                    row = row.child(indicator(true));
                }
                if drop_zone == Some(DropZone::After) && !item.is_group {
                    row = row.child(indicator(false));
                }

                if item.is_group {
                    let group_index = item.original_index as usize;
                    let name = item.group_name.clone();
                    let count = item.group_repo_count;
                    row.on_click(cx.listener(move |this, _, _, cx| {
                        this.update_repo_entries(
                            crate::config::toggle_group_collapsed(group_index),
                            cx,
                        );
                    }))
                    .child(
                        div()
                            .w(px(16.))
                            .text_xs()
                            .text_color(rgb(0x888888))
                            .child(if item.group_collapsed { "▶" } else { "▼" }),
                    )
                    .child(div().w(px(24.)).child("📁"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("{} ({})", item.group_name, item.group_repo_count)),
                    )
                    .child(
                        Button::new(("delete-group", idx))
                            .xsmall()
                            .ghost()
                            .label("×")
                            .tooltip("Delete Group")
                            .on_click(cx.listener(move |_, _, window, cx| {
                                cx.stop_propagation();
                                dialogs::confirm_delete_group(
                                    cx.entity().downgrade(),
                                    group_index,
                                    name.clone(),
                                    count,
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .into_any_element()
                } else {
                    let path = item.repo_path.clone();
                    let delete_path = path.clone();
                    let dragged = DraggedRepo {
                        item_index: idx,
                        name: item.repo_name.clone(),
                    };
                    row.on_drag(dragged, |dragged, _, _, cx| {
                        cx.new(|_| DragPreview {
                            label: dragged.name.clone(),
                        })
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.show_repo_sidebar = false;
                        this.open_repo(&path, cx);
                    }))
                    .when(item.indent_level > 0, |s| s.child(div().w(px(16.))))
                    .child(
                        div()
                            .w(px(28.))
                            .flex_none()
                            .whitespace_nowrap()
                            .text_xs()
                            .text_color(if is_current {
                                white()
                            } else {
                                rgb(colors::ACCENT).into()
                            })
                            .child("</>"),
                    )
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(if is_current {
                                        white()
                                    } else {
                                        rgb(colors::TEXT).into()
                                    })
                                    .child(item.repo_name.clone()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .text_color(rgb(if is_current { 0xdddddd } else { 0x888888 }))
                                    .child(item.repo_path.clone()),
                            ),
                    )
                    .child(
                        Button::new(("remove-repo", idx))
                            .xsmall()
                            .ghost()
                            .label("×")
                            .tooltip("Remove from list")
                            .on_click(cx.listener(move |_, _, window, cx| {
                                cx.stop_propagation();
                                dialogs::confirm_remove_repo(
                                    cx.entity().downgrade(),
                                    idx,
                                    delete_path.clone(),
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .into_any_element()
                }
            })
            .collect::<Vec<_>>();

        // バックドロップ（クリックで閉じる）
        div()
            .id("repo-sidebar-backdrop")
            .absolute()
            .inset_0()
            .bg(rgba(0x00000080))
            .on_click(cx.listener(|this, _, _, cx| {
                this.show_repo_sidebar = false;
                this.drag_target = None;
                cx.notify();
            }))
            .child(
                v_flex()
                    .id("repo-sidebar")
                    .occlude()
                    .w(px(SIDEBAR_WIDTH))
                    .max_w_full()
                    .h_full()
                    .p_1()
                    .gap_1()
                    .bg(rgb(colors::PANEL))
                    .border_r_1()
                    .border_color(rgb(colors::BORDER))
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        h_flex()
                            .h(px(32.))
                            .flex_none()
                            .pl_2()
                            .gap_1()
                            .child(
                                div()
                                    .flex_1()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Repositories"),
                            )
                            .child(
                                Button::new("add-group")
                                    .small()
                                    .ghost()
                                    .label("📁+")
                                    .tooltip("Create Group")
                                    .on_click(cx.listener(|_, _, window, cx| {
                                        dialogs::open_create_group_dialog(
                                            cx.entity().downgrade(),
                                            window,
                                            cx,
                                        )
                                    })),
                            )
                            .child(
                                Button::new("close-repo-list")
                                    .small()
                                    .ghost()
                                    .label("×")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.show_repo_sidebar = false;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        list_container().child(
                            div()
                                .id("repo-list")
                                .size_full()
                                .overflow_y_scrollbar()
                                .child(
                                    v_flex().children(rows).child(
                                        // 末尾へのドロップ領域
                                        div()
                                            .id("repo-list-end")
                                            .h(px(ITEM_HEIGHT))
                                            .drag_over::<DraggedRepo>(|s, _, _, _| {
                                                s.border_t_2().border_color(rgb(0x3584e4))
                                            })
                                            .on_drop(cx.listener(
                                                move |this, dragged: &DraggedRepo, _, cx| {
                                                    this.drop_repo(
                                                        dragged.item_index,
                                                        item_count,
                                                        cx,
                                                    );
                                                },
                                            )),
                                    ),
                                ),
                        ),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .gap_2()
                            .py_1()
                            .child(
                                Button::new("open-local")
                                    .flex_1()
                                    .label("📂 Open Local...")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.show_repo_sidebar = false;
                                        this.browse_repo(cx);
                                        cx.notify();
                                    })),
                            )
                            .child(Button::new("clone").flex_1().label("📥 Clone...").on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.show_repo_sidebar = false;
                                    cx.notify();
                                    dialogs::open_clone_dialog(cx.entity(), window, cx);
                                }),
                            )),
                    ),
            )
    }
}

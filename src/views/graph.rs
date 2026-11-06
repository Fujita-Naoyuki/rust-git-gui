use gpui_kit::component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{h_flex, v_flex, v_virtual_list};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::sidebar::branch_menu;
use super::{colors, dialogs, list_container, status_badge, ROW_HEIGHT};
use crate::app::GitApp;
use crate::graph::{GraphRow, GraphShape, NODE_RADIUS};
use crate::model::{graph_color, CommitData};

const HASH_WIDTH: f32 = 70.0;
const AUTHOR_WIDTH: f32 = 120.0;
const DATE_WIDTH: f32 = 110.0;

/// 1行分のグラフ（線とノード）を描画する
fn graph_canvas(row: GraphRow, commit: &CommitData, width: f32) -> impl IntoElement {
    let node_color = if commit.is_uncommitted {
        rgb(0x808080).into()
    } else {
        rgb(graph_color(commit.color_idx)).into()
    };
    let hollow = commit.is_merge || commit.is_uncommitted;

    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let origin = bounds.origin;
            let at = |(x, y): (f32, f32)| point(origin.x + px(x), origin.y + px(y));

            for segment in &row.segments {
                let mut builder = PathBuilder::stroke(px(2.));
                match segment.shape {
                    GraphShape::Line { from, to } => {
                        builder.move_to(at(from));
                        builder.line_to(at(to));
                    }
                    GraphShape::Curve {
                        from,
                        ctrl_a,
                        ctrl_b,
                        to,
                    } => {
                        builder.move_to(at(from));
                        builder.cubic_bezier_to(at(to), at(ctrl_a), at(ctrl_b));
                    }
                }
                if let Ok(path) = builder.build() {
                    window.paint_path(path, rgb(graph_color(segment.color_idx)));
                }
            }

            // ノード（マージ・未コミットは中抜き）
            let r = NODE_RADIUS + 1.0;
            let center = at((row.node_x, ROW_HEIGHT / 2.0));
            let node_bounds = Bounds::new(
                point(center.x - px(r), center.y - px(r)),
                size(px(r * 2.0), px(r * 2.0)),
            );
            let fill: Hsla = if hollow {
                rgb(colors::BG).into()
            } else {
                node_color
            };
            window.paint_quad(quad(
                node_bounds,
                px(r),
                fill,
                px(2.),
                node_color,
                BorderStyle::Solid,
            ));
        },
    )
    .flex_none()
    .w(px(width))
    .h(px(ROW_HEIGHT))
}

impl GitApp {
    pub(super) fn render_commit_graph(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let header_cell = |text: &'static str, width: Option<f32>| {
            div()
                .when_some(width, |s, w| s.w(px(w)).flex_none())
                .when(width.is_none(), |s| s.flex_1())
                .child(text)
        };

        v_flex()
            .size_full()
            .p_1()
            .gap_0p5()
            .child(
                h_flex()
                    .h(px(22.))
                    .flex_none()
                    .px_1()
                    .rounded(px(2.))
                    .bg(rgb(colors::PANEL))
                    .text_xs()
                    .text_color(rgb(colors::MUTED))
                    .child(header_cell("Graph", Some(self.graph_width)))
                    .child(header_cell("Commit", Some(HASH_WIDTH)))
                    .child(header_cell("Description", None))
                    .child(header_cell("Author", Some(AUTHOR_WIDTH)))
                    .child(header_cell("Date", Some(DATE_WIDTH))),
            )
            .child(
                list_container().child(
                    v_virtual_list(
                        cx.entity(),
                        "commit-list",
                        self.commit_item_sizes.clone(),
                        |this, range, _, cx| {
                            range
                                .map(|ix| this.render_commit_row(ix, cx).into_any_element())
                                .collect()
                        },
                    )
                    .track_scroll(&self.commit_scroll)
                    .size_full(),
                ),
            )
    }

    fn render_commit_row(&self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let commit = &self.snapshot.commits[ix];
        let selected = self.selected_commit == Some(ix);
        let app = cx.entity().downgrade();
        let lane_color = rgb(graph_color(commit.color_idx));

        let badges = commit.branches.iter().enumerate().map(|(bi, branch)| {
            let menu_app = app.clone();
            let name = branch.name.clone();
            let (is_remote, is_current) = (branch.is_remote, branch.is_current);
            h_flex()
                .id(("branch-badge", ix * 64 + bi))
                .flex_none()
                .gap_1()
                .px_1p5()
                .rounded(px(4.))
                .bg(if is_remote {
                    rgb(colors::BORDER)
                } else {
                    lane_color
                })
                .text_xs()
                .text_color(white())
                .child(if is_remote { "☁" } else { "⎇" })
                .child(branch.name.clone())
                .context_menu(move |menu, _, _| {
                    branch_menu(
                        menu,
                        menu_app.clone(),
                        name.clone(),
                        is_remote,
                        is_current,
                        false,
                    )
                })
        });

        let message_color = if commit.is_uncommitted {
            0xc0c080
        } else if selected {
            colors::ACCENT
        } else {
            colors::TEXT
        };
        let muted = if commit.is_uncommitted {
            0x808080
        } else {
            colors::MUTED
        };

        let row = h_flex()
            .id(("commit", ix))
            .w_full()
            .h(px(ROW_HEIGHT))
            .px_1()
            .cursor_pointer()
            .when(selected, |s| s.bg(rgb(colors::SELECTED)))
            .hover(|s| s.bg(rgb(colors::HOVER)))
            .on_click(cx.listener(move |this, _, _, cx| this.select_commit(ix, cx)))
            .child(graph_canvas(commit.graph.clone(), commit, self.graph_width))
            .child(
                div()
                    .w(px(HASH_WIDTH))
                    .flex_none()
                    .font_family(super::mono_font(cx))
                    .text_color(rgb(muted))
                    .child(commit.hash.clone()),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .overflow_hidden()
                    .children(badges)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(rgb(message_color))
                            .child(commit.message.clone()),
                    ),
            )
            .child(
                div()
                    .w(px(AUTHOR_WIDTH))
                    .flex_none()
                    .truncate()
                    .text_color(rgb(muted))
                    .child(commit.author.clone()),
            )
            .child(
                div()
                    .w(px(DATE_WIDTH))
                    .flex_none()
                    .text_color(rgb(colors::MUTED))
                    .child(commit.date.clone()),
            );

        if commit.is_uncommitted {
            return row.into_any_element();
        }

        let hash = commit.full_hash.clone();
        let message = commit.message.clone();
        row.context_menu(move |menu, window, cx| {
            let item = |label: &str, f: Box<dyn Fn(&mut GitApp, &mut Context<GitApp>)>| {
                let app = app.clone();
                PopupMenuItem::new(label.to_string()).on_click(move |_, _, cx| {
                    app.update(cx, |this, cx| f(this, cx)).ok();
                })
            };
            let h = hash.clone();
            let m = message.clone();
            let h2 = hash.clone();
            let h3 = hash.clone();
            let branch_app = app.clone();
            let branch_hash = hash.clone();
            let reset_app = app.clone();
            let reset_hash = hash.clone();

            menu.item(item(
                "Copy Commit Hash",
                Box::new(move |this, cx| this.copy_text(&h, cx)),
            ))
            .item(item(
                "Copy Commit Message",
                Box::new(move |this, cx| this.copy_text(&m, cx)),
            ))
            .item(item(
                "Open on GitHub",
                Box::new(move |this, cx| this.open_commit_on_github(&h2, cx)),
            ))
            .item(
                PopupMenuItem::new("Create Branch Here").on_click(move |_, window, cx| {
                    dialogs::open_create_branch_dialog(
                        branch_app.clone(),
                        Some(branch_hash.clone()),
                        window,
                        cx,
                    );
                }),
            )
            .separator()
            .submenu("Reset to This Commit", window, cx, move |sub, _, _| {
                let reset = |label: &str, mode: &'static str| {
                    let app = reset_app.clone();
                    let hash = reset_hash.clone();
                    PopupMenuItem::new(label.to_string()).on_click(move |_, window, cx| {
                        if mode == "hard" {
                            dialogs::confirm_hard_reset(app.clone(), hash.clone(), window, cx);
                        } else {
                            app.update(cx, |this, cx| this.reset_to_commit(&hash, mode, cx))
                                .ok();
                        }
                    })
                };
                sub.item(reset("Soft (keep changes staged)", "soft"))
                    .item(reset("Mixed (keep changes unstaged)", "mixed"))
                    .item(reset("Hard (discard changes)", "hard"))
            })
            .item(item(
                "Revert Commit",
                Box::new(move |this, cx| this.revert_commit(&h3, cx)),
            ))
        })
        .into_any_element()
    }

    /// 選択コミットの変更ファイル一覧とDiff
    pub(super) fn render_commit_details(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let rows = self
            .diff_files
            .iter()
            .enumerate()
            .map(|(idx, file)| {
                let selected = self.selected_diff_file == Some(idx);
                h_flex()
                    .id(("diff-file", idx))
                    .h(px(ROW_HEIGHT))
                    .px_1()
                    .gap_1()
                    .cursor_pointer()
                    .when(selected, |s| s.bg(rgb(colors::SELECTED)))
                    .hover(|s| s.bg(rgb(colors::HOVER)))
                    .on_click(cx.listener(move |this, _, _, cx| this.select_diff_file(idx, cx)))
                    .child(status_badge(&file.status))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(rgb(if selected {
                                colors::ACCENT
                            } else {
                                colors::TEXT
                            }))
                            .child(file.filename.clone()),
                    )
            })
            .collect::<Vec<_>>();

        h_flex()
            .size_full()
            .bg(rgb(colors::PANEL))
            .p_1()
            .gap_1()
            .child(
                v_flex()
                    .w(px(300.))
                    .flex_none()
                    .h_full()
                    .gap_1()
                    .child(super::section_header(
                        format!("Changed Files ({})", rows.len()),
                        colors::ACCENT,
                    ))
                    .child(
                        list_container().child(
                            div()
                                .id("diff-file-list")
                                .size_full()
                                .overflow_y_scrollbar()
                                .child(v_flex().children(rows)),
                        ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(self.render_diff_view(window, cx)),
            )
    }
}

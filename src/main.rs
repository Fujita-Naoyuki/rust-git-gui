#![windows_subsystem = "windows"]

use arboard::Clipboard;
#[cfg(target_os = "linux")]
use arboard::SetExtLinux;
use chrono::{DateTime, Local, TimeZone};
use git2::{BranchType, DiffOptions, Oid, Repository, StatusOptions};
use slint::{Color, Model, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::rc::Rc;

slint::include_modules!();

// Windowsでコンソールウィンドウを非表示にしてgitコマンドを作成するヘルパー
#[cfg(target_os = "windows")]
fn create_git_command() -> std::process::Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    let mut cmd = std::process::Command::new("git");
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

#[cfg(not(target_os = "windows"))]
fn create_git_command() -> std::process::Command {
    std::process::Command::new("git")
}

// ========== 別スレッドでのDiff計算 ==========

/// 別スレッドでコミットのDiffファイル一覧とDiff内容を計算する
fn compute_commit_diff_in_thread(
    repo_path: String,
    commit_hash: String,
) -> (Vec<DiffFileData>, Vec<DiffLineData>, usize) {
    let Ok(repo) = Repository::open(&repo_path) else {
        return (vec![], vec![], 0);
    };

    if commit_hash.is_empty() {
        return (vec![], vec![], 0);
    }

    let Ok(commit) = repo.find_commit(Oid::from_str(&commit_hash).unwrap_or(Oid::zero())) else {
        return (vec![], vec![], 0);
    };
    let Ok(tree) = commit.tree() else {
        return (vec![], vec![], 0);
    };

    let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());

    let mut opts = DiffOptions::new();
    let Ok(diff) = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut opts))
    else {
        return (vec![], vec![], 0);
    };

    // ファイル一覧を取得
    let mut files = vec![];
    for delta in diff.deltas() {
        let status = match delta.status() {
            git2::Delta::Added => "A",
            git2::Delta::Deleted => "D",
            git2::Delta::Modified => "M",
            git2::Delta::Renamed => "R",
            _ => "?",
        };
        let path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        files.push(DiffFileData {
            filename: path.into(),
            status: status.into(),
        });
    }

    // 最初のファイルのDiff内容を取得
    let (diff_lines, total_count) = if !files.is_empty() {
        let target_path = files[0].filename.to_string();
        let mut opts = DiffOptions::new();
        opts.pathspec(&target_path);
        opts.context_lines(3);

        if let Ok(diff) = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut opts))
        {
            parse_diff_standalone(&diff)
        } else {
            (vec![], 0)
        }
    } else {
        (vec![], 0)
    };

    (files, diff_lines, total_count)
}

/// Diff行数の上限（パフォーマンス対策）
const MAX_DIFF_LINES: usize = 200;
/// カウント上限（これ以上は計算しない）
const MAX_COUNT_LINES: usize = 100000;

/// Diffをパースするスタンドアロン関数
fn parse_diff_standalone(diff: &git2::Diff) -> (Vec<DiffLineData>, usize) {
    use std::cell::Cell;
    let lines = std::rc::Rc::new(std::cell::RefCell::new(vec![]));
    let current_hunk_index = Cell::new(-1i32);
    let truncated = Cell::new(false);
    let total_lines = Cell::new(0usize);
    let stop_processing = Cell::new(false);

    let lines_clone = lines.clone();
    let _ = diff.print(git2::DiffFormat::Patch, |delta, _hunk, line| {
        if stop_processing.get() {
            return false;
        }

        // カウント上限チェック
        if total_lines.get() >= MAX_COUNT_LINES {
            stop_processing.set(true);
            return false;
        }
        total_lines.set(total_lines.get() + 1);

        // 表示上限チェック
        if lines_clone.borrow().len() >= MAX_DIFF_LINES {
            truncated.set(true);
            return true; // カウントのために継続
        }

        let line_type = match line.origin() {
            '+' => "+",
            '-' => "-",
            ' ' => " ",
            'H' | 'F' => "@@",
            _ => "",
        };

        if line.origin() == 'H' {
            current_hunk_index.set(current_hunk_index.get() + 1);
        }

        let old_line_num = line.old_lineno().map(|n| n as i32).unwrap_or(0);
        let new_line_num = line.new_lineno().map(|n| n as i32).unwrap_or(0);

        if let Ok(content) = std::str::from_utf8(line.content()) {
            if line.origin() == 'F' {
                if let Some(path) = delta.new_file().path() {
                    lines_clone.borrow_mut().push(DiffLineData {
                        content: format!("--- {}", path.display()).into(),
                        line_type: "diff".into(),
                        old_line_num: 0,
                        new_line_num: 0,
                        hunk_index: -1,
                    });
                }
            } else {
                let text = content.trim_end_matches('\n');
                if !text.is_empty() || line_type == " " {
                    lines_clone.borrow_mut().push(DiffLineData {
                        content: text.into(),
                        line_type: line_type.into(),
                        old_line_num,
                        new_line_num,
                        hunk_index: current_hunk_index.get(),
                    });
                }
            }
        }
        true
    });

    let mut result = lines.borrow_mut().clone();

    // 切り捨てメッセージを追加
    if truncated.get() {
        result.push(DiffLineData {
            content: format!(
                "... (truncated: diff exceeds {} lines, view on GitHub for full diff)",
                MAX_DIFF_LINES
            )
            .into(),
            line_type: "@@".into(),
            old_line_num: 0,
            new_line_num: 0,
            hunk_index: -1,
        });
    }

    (result, total_lines.get())
}

// ========== リポジトリ履歴管理 ==========

const MAX_RECENT_REPOS: usize = 10;

fn get_config_path() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("git-client")
        .join("recent_repos.json")
}

fn get_commit_history_path() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("git-client")
        .join("commit_history.json")
}

fn load_commit_history() -> Vec<String> {
    let path = get_commit_history_path();
    if let Ok(content) = fs::read_to_string(&path) {
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        Vec::new()
    }
}

fn save_commit_history(history: &[String]) {
    let path = get_commit_history_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(history) {
        let _ = fs::write(&path, json);
    }
}

fn load_recent_repos() -> Vec<String> {
    let path = get_config_path();
    if let Ok(content) = fs::read_to_string(&path) {
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        Vec::new()
    }
}

fn save_recent_repos(repos: &[String]) {
    let path = get_config_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(repos) {
        let _ = fs::write(&path, json);
    }
}

fn add_recent_repo(path: &str) -> Vec<String> {
    let mut repos = load_recent_repos();
    // 既存のエントリを削除
    repos.retain(|p| p != path);
    // 先頭に追加
    repos.insert(0, path.to_string());
    // 最大数を超えたら削除
    repos.truncate(MAX_RECENT_REPOS);
    save_recent_repos(&repos);
    repos
}

/// リポジトリを一覧から削除
fn remove_recent_repo(index: usize) -> Vec<String> {
    let mut repos = load_recent_repos();
    if index < repos.len() {
        repos.remove(index);
        save_recent_repos(&repos);
    }
    repos
}

/// リポジトリの順序を変更
fn reorder_recent_repos(from_idx: usize, to_idx: usize) -> Vec<String> {
    let mut repos = load_recent_repos();
    if from_idx < repos.len() && to_idx <= repos.len() && from_idx != to_idx {
        let item = repos.remove(from_idx);
        let insert_idx = if to_idx > from_idx {
            to_idx - 1
        } else {
            to_idx
        };
        repos.insert(insert_idx.min(repos.len()), item);
        save_recent_repos(&repos);
    }
    repos
}

// クリップボードにテキストをコピー（クロスプラットフォーム対応・非同期）
// Linux: 別スレッドで.wait()を使用してクリップボードマネージャーに内容が渡されるまで待機
// Windows/macOS: クリップボードは同期的に動作するため、通常のset_text()を使用
#[cfg(target_os = "linux")]
fn copy_to_clipboard_async(text: String) {
    std::thread::spawn(move || {
        if let Ok(mut clipboard) = Clipboard::new() {
            let _ = clipboard.set().wait().text(&text);
        }
    });
}

#[cfg(not(target_os = "linux"))]
fn copy_to_clipboard_async(text: String) {
    // Windows/macOSではクリップボードが同期的に動作するため、
    // オブジェクトがドロップされてもデータは保持される
    if let Ok(mut clipboard) = Clipboard::new() {
        let _ = clipboard.set_text(&text);
    }
}

// Graph用の色パレット
const GRAPH_COLORS: [(u8, u8, u8); 16] = [
    (53, 132, 228),  // Blue
    (46, 194, 126),  // Green
    (245, 194, 17),  // Yellow
    (224, 27, 36),   // Red
    (145, 65, 172),  // Purple
    (255, 120, 0),   // Orange
    (0, 184, 212),   // Cyan
    (233, 30, 99),   // Pink
    (79, 195, 247),  // Light Blue
    (129, 199, 132), // Light Green
    (255, 183, 77),  // Light Orange
    (240, 98, 146),  // Light Pink
    (186, 104, 200), // Light Purple
    (77, 182, 172),  // Teal
    (174, 213, 129), // Lime
    (144, 164, 174), // Blue Grey
];

fn get_color(idx: usize) -> Color {
    let (r, g, b) = GRAPH_COLORS[idx % GRAPH_COLORS.len()];
    Color::from_rgb_u8(r, g, b)
}

// ========== Git Graphのデータ構造 ==========

const NULL_VERTEX_ID: i32 = -1;

#[derive(Clone, Copy)]
struct Point {
    x: i32,
    y: i32,
}

#[derive(Clone)]
struct Line {
    p1: Point,
    p2: Point,
    locked_first: bool, // TRUE => 線はp1に固定, FALSE => 線はp2に固定
}

#[derive(Clone)]
struct UnavailablePoint {
    connects_to: i32, // Vertex ID or NULL_VERTEX_ID
    on_branch: usize, // Branch index
}

/// Git GraphのBranchクラス
struct Branch {
    colour: usize,
    end: usize,
    lines: Vec<Line>,
    num_uncommitted: usize,
}

impl Branch {
    fn new(colour: usize) -> Self {
        Self {
            colour,
            end: 0,
            lines: Vec::new(),
            num_uncommitted: 0,
        }
    }

    fn add_line(&mut self, p1: Point, p2: Point, is_committed: bool, locked_first: bool) {
        self.lines.push(Line {
            p1,
            p2,
            locked_first,
        });
        if is_committed {
            if p2.x == 0 && (p2.y as usize) < self.num_uncommitted {
                self.num_uncommitted = p2.y as usize;
            }
        } else {
            self.num_uncommitted += 1;
        }
    }

    fn get_colour(&self) -> usize {
        self.colour
    }

    fn set_end(&mut self, end: usize) {
        self.end = end;
    }
}

/// Git GraphのVertexクラス
struct Vertex {
    id: i32,
    x: i32,
    children: Vec<i32>,
    parents: Vec<i32>,
    next_parent: usize,
    on_branch: Option<usize>, // Branch index
    is_committed: bool,
    is_current: bool,
    next_x: i32,
    connections: Vec<UnavailablePoint>,
}

impl Vertex {
    fn new(id: i32) -> Self {
        Self {
            id,
            x: 0,
            children: Vec::new(),
            parents: Vec::new(),
            next_parent: 0,
            on_branch: None,
            is_committed: true,
            is_current: false,
            next_x: 0,
            connections: Vec::new(),
        }
    }

    fn add_child(&mut self, child_id: i32) {
        self.children.push(child_id);
    }

    fn add_parent(&mut self, parent_id: i32) {
        self.parents.push(parent_id);
    }

    #[allow(dead_code)]
    fn has_parents(&self) -> bool {
        !self.parents.is_empty()
    }

    fn get_next_parent(&self) -> Option<i32> {
        self.parents.get(self.next_parent).copied()
    }

    fn register_parent_processed(&mut self) {
        self.next_parent += 1;
    }

    fn is_merge(&self) -> bool {
        self.parents.len() > 1
    }

    fn add_to_branch(&mut self, branch_idx: usize, x: i32) {
        if self.on_branch.is_none() {
            self.on_branch = Some(branch_idx);
            self.x = x;
        }
    }

    fn is_not_on_branch(&self) -> bool {
        self.on_branch.is_none()
    }

    #[allow(dead_code)]
    fn is_on_this_branch(&self, branch_idx: usize) -> bool {
        self.on_branch == Some(branch_idx)
    }

    fn get_point(&self) -> Point {
        Point {
            x: self.x,
            y: self.id,
        }
    }

    fn get_next_point(&self) -> Point {
        Point {
            x: self.next_x,
            y: self.id,
        }
    }

    fn get_point_connecting_to(&self, vertex_id: i32, on_branch: usize) -> Option<Point> {
        for (i, conn) in self.connections.iter().enumerate() {
            if conn.connects_to == vertex_id && conn.on_branch == on_branch {
                return Some(Point {
                    x: i as i32,
                    y: self.id,
                });
            }
        }
        None
    }

    fn register_unavailable_point(&mut self, x: i32, connects_to: i32, on_branch: usize) {
        if x == self.next_x {
            self.next_x = x + 1;
            // Ensure connections vector is large enough
            while self.connections.len() <= x as usize {
                self.connections.push(UnavailablePoint {
                    connects_to: NULL_VERTEX_ID,
                    on_branch: 0,
                });
            }
            self.connections[x as usize] = UnavailablePoint {
                connects_to,
                on_branch,
            };
        }
    }

    fn get_colour(&self, branches: &[Branch]) -> usize {
        self.on_branch
            .map(|b| branches[b].get_colour())
            .unwrap_or(0)
    }

    fn set_not_committed(&mut self) {
        self.is_committed = false;
    }

    fn set_current(&mut self) {
        self.is_current = true;
    }
}

/// Git Graphのグラフ構築エンジン
struct GraphBuilder {
    vertices: Vec<Vertex>,
    branches: Vec<Branch>,
    available_colours: Vec<usize>,
}

impl GraphBuilder {
    fn new() -> Self {
        Self {
            vertices: Vec::new(),
            branches: Vec::new(),
            available_colours: Vec::new(),
        }
    }

    /// コミットデータからグラフを構築
    fn load_commits(
        &mut self,
        commit_count: usize,
        parent_map: &[(usize, Vec<i32>)],
        head_index: Option<usize>,
        has_uncommitted: bool,
    ) {
        self.vertices.clear();
        self.branches.clear();
        self.available_colours.clear();

        if commit_count == 0 {
            return;
        }

        // 全コミットをVertexとして作成
        for i in 0..commit_count {
            self.vertices.push(Vertex::new(i as i32));
        }

        // 親子関係を設定
        for (idx, parents) in parent_map {
            for &parent_id in parents {
                if parent_id >= 0 && (parent_id as usize) < commit_count {
                    self.vertices[*idx].add_parent(parent_id);
                    self.vertices[parent_id as usize].add_child(*idx as i32);
                } else if parent_id == NULL_VERTEX_ID {
                    self.vertices[*idx].add_parent(NULL_VERTEX_ID);
                }
            }
        }

        // Uncommitted changesの設定
        if has_uncommitted && !self.vertices.is_empty() {
            self.vertices[0].set_not_committed();
        }

        // HEADの設定
        if let Some(head_idx) = head_index {
            if head_idx < self.vertices.len() {
                self.vertices[head_idx].set_current();
            }
        }

        // パスを決定
        let mut i = 0;
        while i < self.vertices.len() {
            if self.vertices[i].get_next_parent().is_some() || self.vertices[i].is_not_on_branch() {
                self.determine_path(i);
            } else {
                i += 1;
            }
        }
    }

    /// Git Graphのdetermine_path()相当 - パス決定アルゴリズム
    fn determine_path(&mut self, start_at: usize) {
        let parent_id = self.vertices[start_at].get_next_parent();

        let last_point = if self.vertices[start_at].is_not_on_branch() {
            self.vertices[start_at].get_next_point()
        } else {
            self.vertices[start_at].get_point()
        };

        if let Some(parent_id) = parent_id {
            if parent_id != NULL_VERTEX_ID
                && self.vertices[start_at].is_merge()
                && !self.vertices[start_at].is_not_on_branch()
                && !self.vertices[parent_id as usize].is_not_on_branch()
            {
                // マージ: 両方の頂点が既にブランチ上にある場合
                self.handle_merge_path(start_at, parent_id, last_point);
            } else {
                // 通常のブランチ
                self.handle_normal_path(start_at, last_point);
            }
        } else {
            // 親がない場合も通常パスとして処理
            self.handle_normal_path(start_at, last_point);
        }
    }

    fn handle_merge_path(&mut self, start_at: usize, parent_id: i32, mut last_point: Point) {
        let parent_branch = self.vertices[parent_id as usize].on_branch.unwrap();
        let vertex_is_committed = self.vertices[start_at].is_committed;
        let mut found_point_to_parent = false;

        for i in (start_at + 1)..self.vertices.len() {
            let cur_point = if let Some(p) =
                self.vertices[i].get_point_connecting_to(parent_id, parent_branch)
            {
                found_point_to_parent = true;
                p
            } else {
                self.vertices[i].get_next_point()
            };

            let locked_first =
                !found_point_to_parent && i != parent_id as usize && last_point.x < cur_point.x;
            self.branches[parent_branch].add_line(
                last_point,
                cur_point,
                vertex_is_committed,
                locked_first,
            );
            self.vertices[i].register_unavailable_point(cur_point.x, parent_id, parent_branch);
            last_point = cur_point;

            if found_point_to_parent {
                self.vertices[start_at].register_parent_processed();
                break;
            }
        }
    }

    fn handle_normal_path(&mut self, start_at: usize, mut last_point: Point) {
        let colour = self.get_available_colour(start_at);
        let branch_idx = self.branches.len();
        self.branches.push(Branch::new(colour));

        let vertex_id = self.vertices[start_at].id;
        self.vertices[start_at].add_to_branch(branch_idx, last_point.x);
        self.vertices[start_at].register_unavailable_point(last_point.x, vertex_id, branch_idx);

        let mut vertex_idx = start_at;
        let mut i = start_at + 1;

        while i < self.vertices.len() {
            let parent_id = self.vertices[vertex_idx].get_next_parent();

            if parent_id.is_none() {
                break;
            }

            let cur_point = if let Some(pid) = parent_id {
                if pid != NULL_VERTEX_ID
                    && pid as usize == i
                    && !self.vertices[i].is_not_on_branch()
                {
                    self.vertices[i].get_point()
                } else {
                    self.vertices[i].get_next_point()
                }
            } else {
                self.vertices[i].get_next_point()
            };

            let vertex_is_committed = self.vertices[vertex_idx].is_committed;
            let locked_first = last_point.x < cur_point.x;
            self.branches[branch_idx].add_line(
                last_point,
                cur_point,
                vertex_is_committed,
                locked_first,
            );

            if let Some(pid) = parent_id {
                self.vertices[i].register_unavailable_point(cur_point.x, pid, branch_idx);
            } else {
                self.vertices[i].register_unavailable_point(
                    cur_point.x,
                    NULL_VERTEX_ID,
                    branch_idx,
                );
            }

            last_point = cur_point;

            // 親に到達したかチェック
            if let Some(pid) = parent_id {
                if pid != NULL_VERTEX_ID && pid as usize == i {
                    self.vertices[vertex_idx].register_parent_processed();
                    let parent_on_branch = !self.vertices[i].is_not_on_branch();
                    self.vertices[i].add_to_branch(branch_idx, cur_point.x);
                    vertex_idx = i;

                    let next_parent = self.vertices[vertex_idx].get_next_parent();
                    if next_parent.is_none() || parent_on_branch {
                        break;
                    }
                }
            }
            i += 1;
        }

        // 最後の頂点で親がNULL_VERTEX_IDの場合
        if i == self.vertices.len() {
            if let Some(pid) = self.vertices[vertex_idx].get_next_parent() {
                if pid == NULL_VERTEX_ID {
                    self.vertices[vertex_idx].register_parent_processed();
                }
            }
        }

        self.branches[branch_idx].set_end(i);
        self.available_colours[colour] = i;
    }

    /// 利用可能な色を取得（Git Graphの色再利用ロジック）
    fn get_available_colour(&mut self, start_at: usize) -> usize {
        for (i, &end) in self.available_colours.iter().enumerate() {
            if start_at > end {
                return i;
            }
        }
        self.available_colours.push(0);
        self.available_colours.len() - 1
    }

    /// SVGパスを生成（線用パスとノード用パスを分離）
    /// 戻り値: (線用パス[8], ノード用パス)
    fn generate_svg_paths(&self, row: usize) -> ([String; 8], String) {
        const COL_SPACING: f32 = 16.0;
        const ROW_HEIGHT: f32 = 28.0;
        const NODE_CENTER_Y: f32 = ROW_HEIGHT / 2.0;
        const CURVE_OFFSET: f32 = ROW_HEIGHT * 0.8;
        const NODE_RADIUS: f32 = 4.0;

        let mut paths: [String; 8] = Default::default();
        let mut node_path = String::new();

        // このコミットを通過する全ブランチの線を描画
        for branch in self.branches.iter() {
            let color_idx = branch.get_colour() % 8;

            for line in &branch.lines {
                // この行に関係する線のみ処理
                if line.p1.y as usize == row
                    || line.p2.y as usize == row
                    || (line.p1.y < row as i32 && line.p2.y > row as i32)
                {
                    let x1 = line.p1.x as f32 * COL_SPACING + 7.0;
                    let y1 = line.p1.y as f32 * ROW_HEIGHT + NODE_CENTER_Y;
                    let x2 = line.p2.x as f32 * COL_SPACING + 7.0;
                    let y2 = line.p2.y as f32 * ROW_HEIGHT + NODE_CENTER_Y;

                    // この行の範囲内の部分のみ描画
                    let row_top = row as f32 * ROW_HEIGHT;
                    let row_bottom = row_top + ROW_HEIGHT;

                    if x1 == x2 {
                        // 垂直線
                        let draw_y1 = y1.max(row_top);
                        let draw_y2 = y2.min(row_bottom);
                        if draw_y1 < draw_y2 {
                            // ローカル座標に変換
                            let local_y1 = draw_y1 - row_top;
                            let local_y2 = draw_y2 - row_top;
                            paths[color_idx]
                                .push_str(&format!("M {} {} L {} {} ", x1, local_y1, x1, local_y2));
                        }
                    } else {
                        // 曲線（この行が始点または終点の場合のみ）
                        if line.p1.y as usize == row || line.p2.y as usize == row {
                            self.draw_curve_segment(
                                &mut paths[color_idx],
                                line,
                                row,
                                COL_SPACING,
                                ROW_HEIGHT,
                                CURVE_OFFSET,
                            );
                        }
                    }
                }
            }
        }

        // ノードをSVGパスとして描画（線と同じ座標系）
        if row < self.vertices.len() {
            let vertex = &self.vertices[row];
            let node_x = vertex.x as f32 * COL_SPACING + 7.0;
            let node_y = NODE_CENTER_Y;

            // 円を描画: M (x-r) y a r r 0 1 0 (2r) 0 a r r 0 1 0 (-2r) 0
            node_path = format!(
                "M {} {} m -{} 0 a {} {} 0 1 0 {} 0 a {} {} 0 1 0 -{} 0 ",
                node_x,
                node_y,
                NODE_RADIUS,
                NODE_RADIUS,
                NODE_RADIUS,
                NODE_RADIUS * 2.0,
                NODE_RADIUS,
                NODE_RADIUS,
                NODE_RADIUS * 2.0
            );
        }

        (paths, node_path)
    }

    fn draw_curve_segment(
        &self,
        path: &mut String,
        line: &Line,
        row: usize,
        col_spacing: f32,
        row_height: f32,
        curve_offset: f32,
    ) {
        let node_center_y = row_height / 2.0;
        let x1 = line.p1.x as f32 * col_spacing + 7.0;
        let x2 = line.p2.x as f32 * col_spacing + 7.0;

        if line.p1.y as usize == row {
            // この行が始点
            let local_y1 = node_center_y;
            let local_y2 = row_height;

            if line.locked_first {
                // 上に固定: 曲線は下に向かう
                let ctrl_y = local_y1 + curve_offset.min(row_height - node_center_y);
                path.push_str(&format!(
                    "M {} {} C {} {} {} {} {} {} ",
                    x1, local_y1, x1, ctrl_y, x2, local_y2, x2, local_y2
                ));
            } else {
                // 下に固定: 直線で下へ、次の行で曲がる
                path.push_str(&format!("M {} {} L {} {} ", x1, local_y1, x1, local_y2));
            }
        } else if line.p2.y as usize == row {
            // この行が終点
            let local_y1 = 0.0;
            let local_y2 = node_center_y;

            if line.locked_first {
                // 上に固定: 直線で上から来る
                path.push_str(&format!("M {} {} L {} {} ", x2, local_y1, x2, local_y2));
            } else {
                // 下に固定: 曲線で終点に向かう
                let ctrl_y = local_y2 - curve_offset.min(node_center_y);
                path.push_str(&format!(
                    "M {} {} C {} {} {} {} {} {} ",
                    x1, local_y1, x1, local_y1, x2, ctrl_y, x2, local_y2
                ));
            }
        }
    }

    fn get_vertex_column(&self, row: usize) -> i32 {
        if row < self.vertices.len() {
            self.vertices[row].x
        } else {
            0
        }
    }

    fn get_vertex_colour(&self, row: usize) -> usize {
        if row < self.vertices.len() {
            self.vertices[row].get_colour(&self.branches)
        } else {
            0
        }
    }

    fn is_vertex_merge(&self, row: usize) -> bool {
        if row < self.vertices.len() {
            self.vertices[row].is_merge()
        } else {
            false
        }
    }

    #[allow(dead_code)]
    fn is_vertex_current(&self, row: usize) -> bool {
        if row < self.vertices.len() {
            self.vertices[row].is_current
        } else {
            false
        }
    }
}

// ========== GitClient ==========

struct GitClient {
    repo: Option<Repository>,
    repo_path: Option<String>,
}

impl GitClient {
    fn new() -> Self {
        Self {
            repo: None,
            repo_path: None,
        }
    }

    fn open_repo(&mut self, path: &str) -> Result<(), String> {
        match Repository::open(path) {
            Ok(repo) => {
                self.repo = Some(repo);
                self.repo_path = Some(path.to_string());
                Ok(())
            }
            Err(e) => Err(format!("Failed to open repository: {}", e)),
        }
    }

    fn get_repo_path(&self) -> Option<String> {
        self.repo_path.clone()
    }

    fn get_current_branch(&self) -> String {
        self.repo.as_ref().map_or("".to_string(), |repo| {
            repo.head()
                .ok()
                .and_then(|h| h.shorthand().map(|s| s.to_string()))
                .unwrap_or_default()
        })
    }

    fn get_local_branches(&self) -> Vec<LocalBranchData> {
        let Some(repo) = &self.repo else {
            return vec![];
        };
        let current = self.get_current_branch();

        let mut branches = vec![];

        if let Ok(branch_iter) = repo.branches(Some(BranchType::Local)) {
            for branch in branch_iter.flatten() {
                if let Some(name) = branch.0.name().ok().flatten() {
                    // リモートとの差分を計算
                    let (ahead, behind) = self.get_ahead_behind(repo, name);

                    branches.push(LocalBranchData {
                        name: name.into(),
                        is_current: name == current,
                        ahead: ahead as i32,
                        behind: behind as i32,
                    });
                }
            }
        }

        branches.sort_by(|a, b| b.is_current.cmp(&a.is_current));
        branches
    }

    /// ローカルブランチとリモートブランチのahead/behindを計算
    fn get_ahead_behind(&self, repo: &Repository, branch_name: &str) -> (usize, usize) {
        // リモートブランチ名を構築（origin/<branch_name>）
        let remote_name = format!("origin/{}", branch_name);

        // ローカルとリモートのOIDを取得
        let local_oid = repo
            .revparse_single(branch_name)
            .ok()
            .and_then(|obj| obj.peel_to_commit().ok())
            .map(|c| c.id());

        let remote_oid = repo
            .revparse_single(&remote_name)
            .ok()
            .and_then(|obj| obj.peel_to_commit().ok())
            .map(|c| c.id());

        match (local_oid, remote_oid) {
            (Some(local), Some(remote)) => repo.graph_ahead_behind(local, remote).unwrap_or((0, 0)),
            _ => (0, 0), // リモートが存在しない場合
        }
    }

    fn get_remote_branches(&self) -> Vec<RemoteBranchData> {
        let Some(repo) = &self.repo else {
            return vec![];
        };

        let mut branches = vec![];

        if let Ok(branch_iter) = repo.branches(Some(BranchType::Remote)) {
            for branch in branch_iter.flatten() {
                if let Some(name) = branch.0.name().ok().flatten() {
                    if !name.ends_with("/HEAD") {
                        branches.push(RemoteBranchData { name: name.into() });
                    }
                }
            }
        }

        branches
    }

    /// Git Graphのアルゴリズムでコミットグラフを構築
    fn get_commits_with_graph(&mut self, limit: usize) -> (Vec<CommitData>, Vec<MergeLineData>) {
        let Some(repo) = &self.repo else {
            return (vec![], vec![]);
        };
        let current_branch = self.get_current_branch();

        // ブランチごとのHEADを取得
        let mut branch_heads: HashMap<String, Vec<String>> = HashMap::new();

        if let Ok(branches) = repo.branches(Some(BranchType::Local)) {
            for branch in branches.flatten() {
                if let (Some(name), Ok(reference)) = (
                    branch.0.name().ok().flatten(),
                    branch.0.get().peel_to_commit(),
                ) {
                    branch_heads
                        .entry(reference.id().to_string())
                        .or_default()
                        .push(name.to_string());
                }
            }
        }
        if let Ok(branches) = repo.branches(Some(BranchType::Remote)) {
            for branch in branches.flatten() {
                if let (Some(name), Ok(reference)) = (
                    branch.0.name().ok().flatten(),
                    branch.0.get().peel_to_commit(),
                ) {
                    if !name.ends_with("/HEAD") {
                        branch_heads
                            .entry(reference.id().to_string())
                            .or_default()
                            .push(name.to_string());
                    }
                }
            }
        }

        let Ok(mut revwalk) = repo.revwalk() else {
            return (vec![], vec![]);
        };
        revwalk
            .set_sorting(git2::Sort::TIME | git2::Sort::TOPOLOGICAL)
            .ok();

        // 全ブランチを追加
        if let Ok(branches) = repo.branches(Some(BranchType::Local)) {
            for branch in branches.flatten() {
                if let Ok(reference) = branch.0.get().peel_to_commit() {
                    let _ = revwalk.push(reference.id());
                }
            }
        }
        if let Ok(branches) = repo.branches(Some(BranchType::Remote)) {
            for branch in branches.flatten() {
                if let Ok(reference) = branch.0.get().peel_to_commit() {
                    let _ = revwalk.push(reference.id());
                }
            }
        }

        // コミットを収集
        let oids: Vec<_> = revwalk.take(limit).flatten().collect();

        // OID -> インデックスのマップを作成
        let mut oid_to_index: HashMap<String, usize> = HashMap::new();
        for (idx, &oid) in oids.iter().enumerate() {
            oid_to_index.insert(oid.to_string(), idx);
        }

        // HEADのインデックスを取得
        let head_oid = repo
            .head()
            .ok()
            .and_then(|h| h.peel_to_commit().ok())
            .map(|c| c.id().to_string());
        let head_index = head_oid.as_ref().and_then(|h| oid_to_index.get(h).copied());

        // 親子関係を構築
        let mut parent_map: Vec<(usize, Vec<i32>)> = Vec::new();
        for (idx, &oid) in oids.iter().enumerate() {
            let Ok(commit) = repo.find_commit(oid) else {
                continue;
            };
            let mut parents = Vec::new();

            for i in 0..commit.parent_count() {
                if let Ok(parent) = commit.parent(i) {
                    let parent_id_str = parent.id().to_string();
                    if let Some(&parent_idx) = oid_to_index.get(&parent_id_str) {
                        parents.push(parent_idx as i32);
                    } else {
                        // 親がグラフ外
                        parents.push(NULL_VERTEX_ID);
                    }
                }
            }
            parent_map.push((idx, parents));
        }

        // Uncommitted changesをチェック
        let (staged, unstaged) = self.get_status();
        let has_uncommitted = !staged.is_empty() || !unstaged.is_empty();

        // グラフを構築
        let mut graph_builder = GraphBuilder::new();

        // Uncommittedがある場合、インデックスを1つずらす
        let commit_offset = if has_uncommitted { 1 } else { 0 };

        // 親マップを調整（Uncommittedを考慮）
        let adjusted_parent_map: Vec<(usize, Vec<i32>)> = parent_map
            .iter()
            .map(|(idx, parents)| {
                let new_idx = idx + commit_offset;
                let new_parents: Vec<i32> = parents
                    .iter()
                    .map(|&p| {
                        if p == NULL_VERTEX_ID {
                            NULL_VERTEX_ID
                        } else {
                            p + commit_offset as i32
                        }
                    })
                    .collect();
                (new_idx, new_parents)
            })
            .collect();

        // Uncommittedの親を追加
        let final_parent_map = if has_uncommitted {
            let uncommitted_parent = vec![];
            let mut map = vec![(0, uncommitted_parent)];
            map.extend(adjusted_parent_map);
            map
        } else {
            adjusted_parent_map
        };

        let total_count = oids.len() + commit_offset;
        let adjusted_head_index = head_index.map(|h| h + commit_offset);

        graph_builder.load_commits(
            total_count,
            &final_parent_map,
            adjusted_head_index,
            has_uncommitted,
        );

        // コミットデータを生成
        let mut commits = vec![];
        let merge_lines = vec![];

        // Uncommitted Changesを先頭に追加
        if has_uncommitted {
            let (svg_paths, node_path) = graph_builder.generate_svg_paths(0);
            let uncommitted = CommitData {
                hash: "*".into(),
                full_hash: "".into(),
                message: SharedString::from(format!(
                    "Uncommitted Changes ({})",
                    staged.len() + unstaged.len()
                )),
                author: "*".into(),
                date: chrono::Local::now()
                    .format("%d %b %H:%M")
                    .to_string()
                    .into(),
                branches: std::rc::Rc::new(slint::VecModel::default()).into(),
                graph_column: graph_builder.get_vertex_column(0),
                graph_color: get_color(0),
                is_merge: false,
                is_head: true,
                is_uncommitted: true,
                svg_path_0: svg_paths[0].clone().into(),
                svg_path_1: svg_paths[1].clone().into(),
                svg_path_2: svg_paths[2].clone().into(),
                svg_path_3: svg_paths[3].clone().into(),
                svg_path_4: svg_paths[4].clone().into(),
                svg_path_5: svg_paths[5].clone().into(),
                svg_path_6: svg_paths[6].clone().into(),
                svg_path_7: svg_paths[7].clone().into(),
                node_path: node_path.into(),
            };
            commits.push(uncommitted);
        }

        // 各コミットのデータを生成
        for (idx, &oid) in oids.iter().enumerate() {
            let Ok(commit) = repo.find_commit(oid) else {
                continue;
            };
            let row = idx + commit_offset;

            let time = commit.time();
            let datetime: DateTime<Local> = Local
                .timestamp_opt(time.seconds(), 0)
                .single()
                .unwrap_or_else(Local::now);
            let oid_str = oid.to_string();

            // ブランチ名
            let branch_names = branch_heads.get(&oid_str).cloned().unwrap_or_default();
            let mut commit_branches = vec![];
            for name in &branch_names {
                let is_current = name == &current_branch;
                let is_remote = name.contains('/');
                commit_branches.push(CommitBranchInfo {
                    name: name.clone().into(),
                    is_current,
                    is_remote,
                });
            }
            commit_branches.sort_by(|a, b| {
                if a.is_current != b.is_current {
                    return b.is_current.cmp(&a.is_current);
                }
                if a.is_remote != b.is_remote {
                    return a.is_remote.cmp(&b.is_remote);
                }
                a.name.cmp(&b.name)
            });
            let branches_model = std::rc::Rc::new(slint::VecModel::from(commit_branches));

            let column = graph_builder.get_vertex_column(row);
            let color_idx = graph_builder.get_vertex_colour(row);
            let is_merge = graph_builder.is_vertex_merge(row);
            let is_head = !branch_names.is_empty();
            let (svg_paths, node_path) = graph_builder.generate_svg_paths(row);

            commits.push(CommitData {
                hash: oid.to_string()[..7].into(),
                full_hash: oid.to_string().into(),
                message: commit.summary().unwrap_or("").into(),
                author: commit.author().name().unwrap_or("").into(),
                date: datetime.format("%d %b %H:%M").to_string().into(),
                branches: branches_model.into(),
                graph_column: column,
                graph_color: get_color(color_idx),
                is_merge,
                is_head,
                is_uncommitted: false,
                svg_path_0: svg_paths[0].clone().into(),
                svg_path_1: svg_paths[1].clone().into(),
                svg_path_2: svg_paths[2].clone().into(),
                svg_path_3: svg_paths[3].clone().into(),
                svg_path_4: svg_paths[4].clone().into(),
                svg_path_5: svg_paths[5].clone().into(),
                svg_path_6: svg_paths[6].clone().into(),
                svg_path_7: svg_paths[7].clone().into(),
                node_path: node_path.into(),
            });
        }

        (commits, merge_lines)
    }

    fn get_status(&self) -> (Vec<FileData>, Vec<FileData>) {
        let Some(repo) = &self.repo else {
            return (vec![], vec![]);
        };

        let mut staged = vec![];
        let mut unstaged = vec![];

        let mut opts = StatusOptions::new();
        opts.include_untracked(true);
        opts.recurse_untracked_dirs(true);

        if let Ok(statuses) = repo.statuses(Some(&mut opts)) {
            for entry in statuses.iter() {
                let path = entry.path().unwrap_or("").to_string();
                let status = entry.status();

                if status.is_index_new() {
                    staged.push(FileData {
                        filename: path.clone().into(),
                        status: "A".into(),
                        staged: true,
                    });
                } else if status.is_index_modified() {
                    staged.push(FileData {
                        filename: path.clone().into(),
                        status: "M".into(),
                        staged: true,
                    });
                } else if status.is_index_deleted() {
                    staged.push(FileData {
                        filename: path.clone().into(),
                        status: "D".into(),
                        staged: true,
                    });
                } else if status.is_index_renamed() {
                    staged.push(FileData {
                        filename: path.clone().into(),
                        status: "R".into(),
                        staged: true,
                    });
                }

                if status.is_wt_new() {
                    unstaged.push(FileData {
                        filename: path.clone().into(),
                        status: "?".into(),
                        staged: false,
                    });
                } else if status.is_wt_modified() {
                    unstaged.push(FileData {
                        filename: path.clone().into(),
                        status: "M".into(),
                        staged: false,
                    });
                } else if status.is_wt_deleted() {
                    unstaged.push(FileData {
                        filename: path.into(),
                        status: "D".into(),
                        staged: false,
                    });
                }
            }
        }
        (staged, unstaged)
    }

    fn stage_file(&self, filename: &str) -> Result<(), String> {
        let Some(repo) = &self.repo else {
            return Err("No repository".into());
        };
        let mut index = repo.index().map_err(|e| e.to_string())?;

        let path = Path::new(filename);
        if path.exists()
            || repo
                .workdir()
                .map(|w| w.join(path).exists())
                .unwrap_or(false)
        {
            index.add_path(path).map_err(|e| e.to_string())?;
        } else {
            index.remove_path(path).map_err(|e| e.to_string())?;
        }
        index.write().map_err(|e| e.to_string())?;
        Ok(())
    }

    fn stage_all(&self) -> Result<(), String> {
        let Some(repo) = &self.repo else {
            return Err("No repository".into());
        };
        let mut index = repo.index().map_err(|e| e.to_string())?;
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .map_err(|e| e.to_string())?;
        index.write().map_err(|e| e.to_string())?;
        Ok(())
    }

    fn unstage_file(&self, filename: &str) -> Result<(), String> {
        let Some(repo) = &self.repo else {
            return Err("No repository".into());
        };
        let head = repo.head().map_err(|e| e.to_string())?;
        let obj = head
            .peel(git2::ObjectType::Commit)
            .map_err(|e| e.to_string())?;
        repo.reset_default(Some(&obj), [Path::new(filename)])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn unstage_all(&self) -> Result<(), String> {
        let Some(repo) = &self.repo else {
            return Err("No repository".into());
        };
        let head = repo.head().map_err(|e| e.to_string())?;
        let obj = head
            .peel(git2::ObjectType::Commit)
            .map_err(|e| e.to_string())?;
        repo.reset_default(Some(&obj), ["*"])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn commit(&self, message: &str) -> Result<(), String> {
        let Some(repo) = &self.repo else {
            return Err("No repository".into());
        };

        let mut index = repo.index().map_err(|e| e.to_string())?;
        let oid = index.write_tree().map_err(|e| e.to_string())?;
        let tree = repo.find_tree(oid).map_err(|e| e.to_string())?;

        let sig = repo.signature().map_err(|e| e.to_string())?;
        let head = repo.head().map_err(|e| e.to_string())?;
        let parent = head.peel_to_commit().map_err(|e| e.to_string())?;

        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn checkout_branch(&self, name: &str) -> Result<(), String> {
        let Some(repo) = &self.repo else {
            return Err("No repository".into());
        };

        let obj = repo
            .revparse_single(&format!("refs/heads/{}", name))
            .map_err(|e| e.to_string())?;

        let mut opts = git2::build::CheckoutBuilder::new();
        opts.safe();

        repo.checkout_tree(&obj, Some(&mut opts))
            .map_err(|e| e.to_string())?;
        repo.set_head(&format!("refs/heads/{}", name))
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn checkout_remote_branch(&self, remote_name: &str) -> Result<(), String> {
        let Some(repo) = &self.repo else {
            return Err("No repository".into());
        };

        // リモートブランチ名から "origin/branch-name" の形式でローカルブランチ名を抽出
        let local_name = remote_name.split('/').skip(1).collect::<Vec<_>>().join("/");
        if local_name.is_empty() {
            return Err("Invalid remote branch name".into());
        }

        // 既にローカルブランチが存在するかチェック
        if repo.find_branch(&local_name, BranchType::Local).is_ok() {
            // 既存のローカルブランチにチェックアウト
            return self.checkout_branch(&local_name);
        }

        // リモートブランチのコミットを取得
        let remote_ref = format!("refs/remotes/{}", remote_name);
        let obj = repo
            .revparse_single(&remote_ref)
            .map_err(|e| e.to_string())?;
        let commit = obj.peel_to_commit().map_err(|e| e.to_string())?;

        // 新しいローカルブランチを作成
        repo.branch(&local_name, &commit, false)
            .map_err(|e| e.to_string())?;

        // 作成したブランチにチェックアウト
        self.checkout_branch(&local_name)
    }

    fn create_branch(&self, name: &str) -> Result<(), String> {
        let Some(repo) = &self.repo else {
            return Err("No repository".into());
        };

        let head = repo.head().map_err(|e| e.to_string())?;
        let commit = head.peel_to_commit().map_err(|e| e.to_string())?;

        repo.branch(name, &commit, false)
            .map_err(|e| e.to_string())?;
        self.checkout_branch(name)?;
        Ok(())
    }

    fn delete_branch(&self, name: &str) -> Result<(), String> {
        let Some(repo) = &self.repo else {
            return Err("No repository".into());
        };

        let mut branch = repo
            .find_branch(name, BranchType::Local)
            .map_err(|e| e.to_string())?;
        branch.delete().map_err(|e| e.to_string())?;
        Ok(())
    }

    fn merge_branch(&self, name: &str) -> Result<(), String> {
        let Some(repo) = &self.repo else {
            return Err("No repository".into());
        };

        let branch = repo
            .find_branch(name, BranchType::Local)
            .map_err(|e| e.to_string())?;
        let reference = branch.get();
        let annotated = repo
            .reference_to_annotated_commit(reference)
            .map_err(|e| e.to_string())?;

        let (analysis, _) = repo
            .merge_analysis(&[&annotated])
            .map_err(|e| e.to_string())?;

        if analysis.is_up_to_date() {
            return Ok(());
        }

        if analysis.is_fast_forward() {
            let refname = format!("refs/heads/{}", self.get_current_branch());
            let mut reference = repo.find_reference(&refname).map_err(|e| e.to_string())?;
            reference
                .set_target(annotated.id(), "Fast-forward")
                .map_err(|e| e.to_string())?;
            repo.set_head(&refname).map_err(|e| e.to_string())?;
            repo.checkout_head(Some(git2::build::CheckoutBuilder::default().force()))
                .map_err(|e| e.to_string())?;
        } else {
            return Err("Merge requires manual resolution".into());
        }

        Ok(())
    }

    fn get_stashes(&mut self) -> Vec<StashData> {
        let Some(repo) = &mut self.repo else {
            return vec![];
        };
        let mut stashes = vec![];
        let mut stash_idx = 0;
        let _ = repo.stash_foreach(|index, name, _oid| {
            stashes.push(StashData {
                index: index as i32,
                message: name.into(),
            });
            stash_idx += 1;
            true
        });
        stashes
    }

    fn stash_save(&mut self, message: &str, include_untracked: bool) -> Result<(), String> {
        let Some(repo) = &mut self.repo else {
            return Err("No repository".into());
        };
        let signature = repo.signature().map_err(|e| e.to_string())?;
        let mut flags = git2::StashFlags::DEFAULT;
        if include_untracked {
            flags.insert(git2::StashFlags::INCLUDE_UNTRACKED);
        }
        repo.stash_save(&signature, message, Some(flags))
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    fn stash_apply(&mut self, index: usize) -> Result<(), String> {
        let Some(repo) = &mut self.repo else {
            return Err("No repository".into());
        };
        let mut options = git2::StashApplyOptions::default();
        repo.stash_apply(index, Some(&mut options))
            .map_err(|e| e.to_string())
    }

    fn stash_pop(&mut self, index: usize) -> Result<(), String> {
        let Some(repo) = &mut self.repo else {
            return Err("No repository".into());
        };
        let mut options = git2::StashApplyOptions::default();
        repo.stash_pop(index, Some(&mut options))
            .map_err(|e| e.to_string())
    }

    fn stash_drop(&mut self, index: usize) -> Result<(), String> {
        let Some(repo) = &mut self.repo else {
            return Err("No repository".into());
        };
        repo.stash_drop(index).map_err(|e| e.to_string())
    }

    fn get_commit_file_diff(&self, oid: &str, file_index: usize) -> (Vec<DiffLineData>, usize) {
        let Some(repo) = &self.repo else {
            return (vec![], 0);
        };

        if oid.is_empty() {
            return (vec![], 0);
        }

        let Ok(commit) = repo.find_commit(Oid::from_str(oid).unwrap_or(Oid::zero())) else {
            return (vec![], 0);
        };
        let Ok(tree) = commit.tree() else {
            return (vec![], 0);
        };

        let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());

        let mut opts = DiffOptions::new();
        let Ok(diff) = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut opts))
        else {
            return (vec![], 0);
        };

        let deltas: Vec<_> = diff.deltas().collect();
        if file_index >= deltas.len() {
            return (vec![], 0);
        }

        let target_path = deltas[file_index]
            .new_file()
            .path()
            .or_else(|| deltas[file_index].old_file().path())
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        let mut opts = DiffOptions::new();
        opts.pathspec(&target_path);
        opts.context_lines(3);

        let Ok(diff) = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut opts))
        else {
            return (vec![], 0);
        };

        self.parse_diff(&diff)
    }

    fn get_file_diff(&self, filename: &str, staged: bool) -> (Vec<DiffLineData>, usize) {
        let Some(repo) = &self.repo else {
            return (vec![], 0);
        };

        let mut opts = DiffOptions::new();
        opts.pathspec(filename);
        opts.context_lines(3);

        let diff = if staged {
            let Ok(head_tree) = repo.head().and_then(|h| h.peel_to_tree()) else {
                return (vec![], 0);
            };
            repo.diff_tree_to_index(Some(&head_tree), None, Some(&mut opts))
        } else {
            // Include untracked files in diff
            opts.include_untracked(true);

            repo.diff_index_to_workdir(None, Some(&mut opts))
        };

        match diff {
            Ok(d) => {
                let (lines, total_lines) = self.parse_diff(&d);
                // If no diff lines but it's an unstaged file, it might be untracked (new file)
                // Read the file content directly and show as all additions
                if lines.is_empty() && !staged {
                    let lines = self.get_new_file_diff(repo, filename);
                    let count = lines.len();
                    return (lines, count);
                }
                (lines, total_lines)
            }
            Err(_) => {
                // If diff failed and it's unstaged, try reading as new file
                if !staged {
                    let lines = self.get_new_file_diff(repo, filename);
                    let count = lines.len();
                    return (lines, count);
                }
                (vec![], 0)
            }
        }
    }

    /// Get diff for a new (untracked) file by reading its contents
    fn get_new_file_diff(&self, repo: &Repository, filename: &str) -> Vec<DiffLineData> {
        let workdir = match repo.workdir() {
            Some(w) => w,
            None => return vec![],
        };

        let file_path = workdir.join(filename);
        let content = match fs::read_to_string(&file_path) {
            Ok(c) => c,
            Err(_) => {
                // Try reading as binary and show a placeholder message
                match fs::read(&file_path) {
                    Ok(_) => {
                        return vec![DiffLineData {
                            content: "(Binary file)".into(),
                            line_type: " ".into(),
                            old_line_num: 0,
                            new_line_num: 0,
                            hunk_index: 0,
                        }]
                    }
                    Err(_) => return vec![],
                }
            }
        };

        let mut lines = vec![];

        // Add file header
        lines.push(DiffLineData {
            content: format!("--- /dev/null").into(),
            line_type: "diff".into(),
            old_line_num: 0,
            new_line_num: 0,
            hunk_index: -1,
        });
        lines.push(DiffLineData {
            content: format!("+++ {}", filename).into(),
            line_type: "diff".into(),
            old_line_num: 0,
            new_line_num: 0,
            hunk_index: -1,
        });

        // Add hunk header
        let line_count = content.lines().count();
        lines.push(DiffLineData {
            content: format!("@@ -0,0 +1,{} @@", line_count).into(),
            line_type: "@@".into(),
            old_line_num: 0,
            new_line_num: 0,
            hunk_index: 0,
        });

        // Add all lines as additions
        for (i, line) in content.lines().enumerate() {
            lines.push(DiffLineData {
                content: format!("+{}", line).into(),
                line_type: "+".into(),
                old_line_num: 0,
                new_line_num: (i + 1) as i32,
                hunk_index: 0,
            });
        }

        lines
    }

    fn parse_diff(&self, diff: &git2::Diff) -> (Vec<DiffLineData>, usize) {
        use std::cell::Cell;
        let lines = Rc::new(RefCell::new(vec![]));
        let current_hunk_index = Cell::new(-1i32);
        let truncated = Cell::new(false);
        let total_lines = Cell::new(0usize);
        let stop_processing = Cell::new(false);

        let lines_clone = lines.clone();
        let _ = diff.print(git2::DiffFormat::Patch, |delta, _hunk, line| {
            if stop_processing.get() {
                return false;
            }

            // カウント上限チェック
            if total_lines.get() >= MAX_COUNT_LINES {
                stop_processing.set(true);
                return false;
            }
            total_lines.set(total_lines.get() + 1);

            // 表示上限チェック
            if lines_clone.borrow().len() >= MAX_DIFF_LINES {
                truncated.set(true);
                return true; // カウントのために継続
            }

            let line_type = match line.origin() {
                '+' => "+",
                '-' => "-",
                ' ' => " ",
                'H' | 'F' => "@@",
                _ => "",
            };

            if line.origin() == 'H' {
                current_hunk_index.set(current_hunk_index.get() + 1);
            }

            let old_line_num = line.old_lineno().map(|n| n as i32).unwrap_or(0);
            let new_line_num = line.new_lineno().map(|n| n as i32).unwrap_or(0);

            if let Ok(content) = std::str::from_utf8(line.content()) {
                if line.origin() == 'F' {
                    if let Some(path) = delta.new_file().path() {
                        lines_clone.borrow_mut().push(DiffLineData {
                            content: format!("--- {}", path.display()).into(),
                            line_type: "diff".into(),
                            old_line_num: 0,
                            new_line_num: 0,
                            hunk_index: -1,
                        });
                    }
                } else {
                    let text = content.trim_end_matches('\n');
                    if !text.is_empty() || line_type == " " {
                        lines_clone.borrow_mut().push(DiffLineData {
                            content: text.into(),
                            line_type: line_type.into(),
                            old_line_num,
                            new_line_num,
                            hunk_index: current_hunk_index.get(),
                        });
                    }
                }
            }
            true
        });

        let mut result = lines.borrow_mut().clone();

        // 切り捨てメッセージを追加
        if truncated.get() {
            result.push(DiffLineData {
                content: format!(
                    "... (truncated: diff exceeds {} lines, view on GitHub for full diff)",
                    MAX_DIFF_LINES
                )
                .into(),
                line_type: "@@".into(),
                old_line_num: 0,
                new_line_num: 0,
                hunk_index: -1,
            });
        }

        (result, total_lines.get())
    }

    /// 特定のHunkをステージングする
    fn stage_hunk(&self, filename: &str, hunk_index: usize) -> Result<(), String> {
        let Some(repo) = &self.repo else {
            return Err("No repository".into());
        };

        // Unstaged diffを取得
        let mut opts = DiffOptions::new();
        opts.pathspec(filename);
        opts.context_lines(3);

        let diff = repo
            .diff_index_to_workdir(None, Some(&mut opts))
            .map_err(|e| e.to_string())?;

        // Hunkを数えて対象のHunkを特定
        let mut current_hunk = 0;
        let mut target_hunk_header = String::new();
        let mut target_hunk_lines: Vec<String> = vec![];
        let mut in_target_hunk = false;

        let _ = diff.print(git2::DiffFormat::Patch, |_delta, hunk, line| {
            match line.origin() {
                'H' => {
                    // Hunkヘッダー
                    if current_hunk == hunk_index {
                        in_target_hunk = true;
                        if let Some(h) = hunk {
                            if let Ok(header) = std::str::from_utf8(h.header()) {
                                target_hunk_header = header.trim_end().to_string();
                            }
                        }
                    } else if in_target_hunk {
                        in_target_hunk = false;
                    }
                    current_hunk += 1;
                }
                '+' | '-' | ' ' => {
                    if in_target_hunk {
                        if let Ok(content) = std::str::from_utf8(line.content()) {
                            target_hunk_lines.push(format!("{}{}", line.origin(), content));
                        }
                    }
                }
                _ => {}
            }
            true
        });

        if target_hunk_header.is_empty() {
            return Err("Hunk not found".into());
        }

        // パッチを生成
        let patch = format!(
            "diff --git a/{filename} b/{filename}\n--- a/{filename}\n+++ b/{filename}\n{}\n{}",
            target_hunk_header,
            target_hunk_lines.join("")
        );

        // git applyでパッチを適用（--cachedでインデックスに適用）
        use std::io::Write;
        let workdir = repo.workdir().ok_or("No workdir")?;
        let mut child = create_git_command()
            .args(["apply", "--cached", "-"])
            .current_dir(workdir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(patch.as_bytes())
                .map_err(|e| e.to_string())?;
        }

        let output = child.wait_with_output().map_err(|e| e.to_string())?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("Failed to stage hunk: {}", stderr));
        }

        Ok(())
    }

    fn discard_file(&self, filename: &str) -> Result<(), String> {
        let Some(repo) = &self.repo else {
            return Err("No repository".into());
        };

        // Check if the file is untracked (new file)
        let mut opts = StatusOptions::new();
        opts.include_untracked(true);
        opts.recurse_untracked_dirs(true);

        if let Ok(statuses) = repo.statuses(Some(&mut opts)) {
            for entry in statuses.iter() {
                let path = entry.path().unwrap_or("");
                // Compare the path with the filename
                if path == filename {
                    let status = entry.status();
                    if status.is_wt_new() {
                        // Untracked file - delete it directly
                        let workdir = repo.workdir().ok_or("No workdir")?;
                        let file_path = workdir.join(filename);
                        fs::remove_file(&file_path)
                            .map_err(|e| format!("Failed to delete file: {}", e))?;
                        return Ok(());
                    }
                    break;
                }
            }
        }

        // For tracked files, restore from HEAD
        let mut checkout_opts = git2::build::CheckoutBuilder::new();
        checkout_opts.path(filename);
        checkout_opts.force();

        repo.checkout_head(Some(&mut checkout_opts))
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// リモートにプッシュ（git pushコマンドを使用）
    /// upstreamがないブランチでも自動的にupstreamを設定する
    fn push(&self) -> Result<(), String> {
        let Some(repo) = &self.repo else {
            return Err("No repository".into());
        };

        let workdir = repo.workdir().ok_or("No workdir")?;
        let branch = self.get_current_branch();
        if branch.is_empty() {
            return Err("No current branch".into());
        }

        let output = create_git_command()
            .args(["push", "-u", "origin", &branch])
            .current_dir(workdir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .map_err(|e| e.to_string())?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("Push failed: {}", stderr));
        }

        Ok(())
    }

    /// リモートからプル（git pullコマンドを使用）
    fn pull(&self) -> Result<(), String> {
        let Some(repo) = &self.repo else {
            return Err("No repository".into());
        };

        let workdir = repo.workdir().ok_or("No workdir")?;
        let output = create_git_command()
            .args(["pull"])
            .current_dir(workdir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .map_err(|e| e.to_string())?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("Pull failed: {}", stderr));
        }

        Ok(())
    }

    /// GitHubのリポジトリURLを取得
    fn get_github_url(&self) -> Option<String> {
        let repo = self.repo.as_ref()?;
        let remote = repo.find_remote("origin").ok()?;
        let url = remote.url()?;

        // SSH形式 (git@github.com:user/repo.git) をHTTPS形式に変換
        if url.starts_with("git@github.com:") {
            let path = url
                .strip_prefix("git@github.com:")?
                .strip_suffix(".git")
                .unwrap_or(url.strip_prefix("git@github.com:")?);
            return Some(format!("https://github.com/{}", path));
        }

        // HTTPS形式 (https://github.com/user/repo.git)
        if url.starts_with("https://github.com/") {
            let clean_url = url.strip_suffix(".git").unwrap_or(url);
            return Some(clean_url.to_string());
        }

        None
    }

    /// Pull Request作成URLを生成
    fn get_pull_request_url(&self, branch_name: &str) -> Option<String> {
        let github_url = self.get_github_url()?;
        // GitHub PR作成URL: https://github.com/user/repo/compare/main...branch?expand=1
        Some(format!(
            "{}/compare/main...{}?expand=1",
            github_url, branch_name
        ))
    }

    /// コミットのGitHub URLを生成
    fn get_commit_github_url(&self, commit_hash: &str) -> Option<String> {
        let github_url = self.get_github_url()?;
        Some(format!("{}/commit/{}", github_url, commit_hash))
    }

    /// 指定したコミットにリセット
    fn reset_to_commit(&self, commit_hash: &str, mode: &str) -> Result<(), String> {
        let Some(repo) = &self.repo else {
            return Err("No repository".into());
        };

        let obj = repo
            .revparse_single(commit_hash)
            .map_err(|e| e.to_string())?;
        let commit = obj.peel_to_commit().map_err(|e| e.to_string())?;

        let reset_type = match mode {
            "soft" => git2::ResetType::Soft,
            "hard" => git2::ResetType::Hard,
            _ => git2::ResetType::Mixed,
        };

        repo.reset(commit.as_object(), reset_type, None)
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// コミットをリバート（打ち消しコミットを作成）
    fn revert_commit(&self, commit_hash: &str) -> Result<(), String> {
        let Some(repo) = &self.repo else {
            return Err("No repository".into());
        };

        let obj = repo
            .revparse_single(commit_hash)
            .map_err(|e| e.to_string())?;
        let commit = obj.peel_to_commit().map_err(|e| e.to_string())?;

        // リバートを実行
        let mut revert_opts = git2::RevertOptions::new();
        repo.revert(&commit, Some(&mut revert_opts))
            .map_err(|e| e.to_string())?;

        // 自動コミット
        let sig = repo.signature().map_err(|e| e.to_string())?;
        let mut index = repo.index().map_err(|e| e.to_string())?;
        let tree_oid = index.write_tree().map_err(|e| e.to_string())?;
        let tree = repo.find_tree(tree_oid).map_err(|e| e.to_string())?;
        let head = repo.head().map_err(|e| e.to_string())?;
        let parent = head.peel_to_commit().map_err(|e| e.to_string())?;

        let message = format!("Revert \"{}\"", commit.summary().unwrap_or(""));
        repo.commit(Some("HEAD"), &sig, &sig, &message, &tree, &[&parent])
            .map_err(|e| e.to_string())?;

        Ok(())
    }

    /// インデックスからコミットハッシュを取得
    fn get_commit_hash_by_index(&self, index: usize) -> Option<String> {
        let repo = self.repo.as_ref()?;
        let mut revwalk = repo.revwalk().ok()?;
        revwalk
            .set_sorting(git2::Sort::TIME | git2::Sort::TOPOLOGICAL)
            .ok();

        // 全ブランチを追加
        if let Ok(branches) = repo.branches(Some(BranchType::Local)) {
            for branch in branches.flatten() {
                if let Ok(reference) = branch.0.get().peel_to_commit() {
                    let _ = revwalk.push(reference.id());
                }
            }
        }
        if let Ok(branches) = repo.branches(Some(BranchType::Remote)) {
            for branch in branches.flatten() {
                if let Ok(reference) = branch.0.get().peel_to_commit() {
                    let _ = revwalk.push(reference.id());
                }
            }
        }

        // Uncommitted changesをチェック
        let (staged, unstaged) = self.get_status();
        let has_uncommitted = !staged.is_empty() || !unstaged.is_empty();

        // Uncommittedの場合はNone
        if has_uncommitted && index == 0 {
            return None;
        }

        let actual_index = if has_uncommitted { index - 1 } else { index };
        let oids: Vec<_> = revwalk.take(actual_index + 1).flatten().collect();
        oids.get(actual_index).map(|oid| oid.to_string())
    }
}

fn main() -> Result<(), slint::PlatformError> {
    let ui = MainWindow::new()?;
    let git_client = Rc::new(RefCell::new(GitClient::new()));

    // コミットメッセージ履歴を読み込み（最大10件保持）
    let loaded_history = load_commit_history();
    let history_model: Vec<SharedString> = loaded_history
        .iter()
        .map(|s| SharedString::from(s.as_str()))
        .collect();
    ui.set_commit_message_history(ModelRc::new(VecModel::from(history_model)));

    let commit_message_history: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(loaded_history));
    const MAX_COMMIT_HISTORY: usize = 10;

    // 最近使用したリポジトリを読み込み
    let recent_repos = load_recent_repos();
    let recent_model: Vec<SharedString> = recent_repos
        .iter()
        .map(|s| SharedString::from(s.as_str()))
        .collect();
    ui.set_recent_repos(ModelRc::new(VecModel::from(recent_model)));

    // 履歴があれば最初のリポジトリを選択、なければホームディレクトリ
    let initial_repo = if !recent_repos.is_empty() {
        ui.set_repo_path(recent_repos[0].clone().into());
        ui.set_selected_repo_index(0);
        Some(recent_repos[0].clone())
    } else if let Some(home) = dirs::home_dir() {
        ui.set_repo_path(home.to_string_lossy().to_string().into());
        None
    } else {
        None
    };

    let refresh_ui = {
        let ui_weak = ui.as_weak();
        let git_client = git_client.clone();
        move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let mut client = git_client.borrow_mut();

            ui.set_current_branch(client.get_current_branch().into());
            ui.set_local_branches(
                Rc::new(slint::VecModel::from(client.get_local_branches())).into(),
            );
            ui.set_remote_branches(
                Rc::new(slint::VecModel::from(client.get_remote_branches())).into(),
            );
            ui.set_stashes(Rc::new(slint::VecModel::from(client.get_stashes())).into());
            let (commits, merge_lines) = client.get_commits_with_graph(300);
            ui.set_commits(Rc::new(slint::VecModel::from(commits)).into());
            ui.set_merge_lines(Rc::new(slint::VecModel::from(merge_lines)).into());

            let (staged, unstaged) = client.get_status();
            let staged_len = staged.len();
            let unstaged_len = unstaged.len();
            ui.set_staged_files(Rc::new(slint::VecModel::from(staged)).into());
            ui.set_unstaged_files(Rc::new(slint::VecModel::from(unstaged)).into());

            // チェック状態をリセット
            ui.set_staged_checked(Rc::new(slint::VecModel::from(vec![false; staged_len])).into());
            ui.set_unstaged_checked(
                Rc::new(slint::VecModel::from(vec![false; unstaged_len])).into(),
            );
            ui.set_staged_checked_count(0);
            ui.set_unstaged_checked_count(0);
            ui.set_last_clicked_staged(-1);
            ui.set_last_clicked_unstaged(-1);

            ui.set_selected_commit(-1);
            ui.set_selected_commit_hash("".into());
            ui.set_selected_file(-1);
            ui.set_diff_lines(Rc::new(slint::VecModel::from(Vec::<DiffLineData>::new())).into());
        }
    };

    // Open repository
    {
        let git_client = git_client.clone();
        let refresh = refresh_ui.clone();
        let ui_weak = ui.as_weak();
        ui.on_open_repo(move |path| {
            let mut client = git_client.borrow_mut();
            match client.open_repo(&path) {
                Ok(()) => {
                    drop(client);
                    // 履歴を更新
                    let repos = add_recent_repo(&path);
                    if let Some(ui) = ui_weak.upgrade() {
                        let recent_model: Vec<SharedString> = repos
                            .iter()
                            .map(|s| SharedString::from(s.as_str()))
                            .collect();
                        ui.set_recent_repos(ModelRc::new(VecModel::from(recent_model)));
                        ui.set_selected_repo_index(0);

                        // リポジトリ名を設定
                        let repo_name = Path::new(&path)
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or(&path)
                            .to_string();
                        ui.set_repo_name(SharedString::from(repo_name));

                        ui.set_status_message("Repository opened".into());
                    }
                    refresh();
                }
                Err(e) => {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_status_message(SharedString::from(format!("Error: {}", e)));
                    }
                }
            }
        });
    }

    // Remove repository from recent list
    {
        let ui_weak = ui.as_weak();
        ui.on_remove_repo(move |index| {
            let repos = remove_recent_repo(index as usize);
            if let Some(ui) = ui_weak.upgrade() {
                let recent_model: Vec<SharedString> = repos
                    .iter()
                    .map(|s| SharedString::from(s.as_str()))
                    .collect();
                ui.set_recent_repos(ModelRc::new(VecModel::from(recent_model)));
            }
        });
    }

    // Reorder repositories in recent list
    {
        let ui_weak = ui.as_weak();
        ui.on_reorder_repos(move |from_idx, to_idx| {
            let repos = reorder_recent_repos(from_idx as usize, to_idx as usize);
            if let Some(ui) = ui_weak.upgrade() {
                let recent_model: Vec<SharedString> = repos
                    .iter()
                    .map(|s| SharedString::from(s.as_str()))
                    .collect();
                ui.set_recent_repos(ModelRc::new(VecModel::from(recent_model)));
            }
        });
    }

    // Browse destination path for clone
    {
        let ui_weak = ui.as_weak();
        ui.on_browse_clone_path(move || {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Select Destination Folder")
                .pick_folder()
            {
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_clone_path(path.to_string_lossy().to_string().into());
                }
            }
        });
    }

    // Clone repository
    {
        let ui_weak = ui.as_weak();
        ui.on_clone_repo(move |url, path| {
            let url = url.to_string();
            let mut path_str = path.to_string();
            let ui_weak_clone = ui_weak.clone();

            std::thread::spawn(move || {
                // スマートパス補完: 指定されたパスが存在し、かつ空でない場合
                let path = Path::new(&path_str);
                if path.exists()
                    && path
                        .read_dir()
                        .map(|mut i| i.next().is_some())
                        .unwrap_or(false)
                {
                    // URLからリポジトリ名を抽出 (e.g. https://github.com/user/repo.git -> repo)
                    let repo_name = url
                        .split('/')
                        .last()
                        .map(|s| s.trim_end_matches(".git"))
                        .unwrap_or("repository");

                    // パスにリポジトリ名を追加
                    let new_path = path.join(repo_name);
                    path_str = new_path.to_string_lossy().to_string();
                }

                // git cloneコマンドを実行（push/pull/fetchと同様にシステムのgitを使用）
                let output = create_git_command()
                    .args(["clone", &url, &path_str])
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .output();

                match output {
                    Ok(out) if out.status.success() => {
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak_clone.upgrade() {
                                ui.set_is_cloning(false);
                                ui.set_show_clone_dialog(false);
                                ui.set_status_message("Clone successful".into());
                                // Open the new repo using existing logic
                                ui.invoke_open_repo(path_str.into());
                            }
                        });
                    }
                    Ok(out) => {
                        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak_clone.upgrade() {
                                ui.set_is_cloning(false);
                                ui.set_clone_error(stderr.into());
                            }
                        });
                    }
                    Err(e) => {
                        let error_msg = e.to_string();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak_clone.upgrade() {
                                ui.set_is_cloning(false);
                                ui.set_clone_error(error_msg.into());
                            }
                        });
                    }
                }
            });
        });
    }

    // Browse repository (folder dialog)
    {
        let ui_weak = ui.as_weak();
        ui.on_browse_repo(move || {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Select Git Repository")
                .pick_folder()
            {
                if let Some(ui) = ui_weak.upgrade() {
                    let path_str = path.to_string_lossy().to_string();
                    ui.set_repo_path(path_str.clone().into());
                    ui.invoke_open_repo(path_str.into());
                }
            }
        });
    }

    // Refresh (非同期Fetch後にUI更新)
    {
        let git_client = git_client.clone();
        let ui_weak = ui.as_weak();
        ui.on_refresh(move || {
            let ui_weak_clone = ui_weak.clone();
            // 「Refreshing...」を表示
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_status_message("Refresh & Fetch: Fetching...".into());
            }

            // リポジトリパスを取得（別スレッドで使用するため）
            let repo_path = git_client.borrow().get_repo_path();

            // 別スレッドでFetchを実行
            std::thread::spawn(move || {
                let fetch_result = if let Some(path) = repo_path {
                    // GitClientを一時的に作成してfetchを実行
                    let output = create_git_command()
                        .args(["fetch", "--all"])
                        .current_dir(&path)
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::piped())
                        .output();

                    match output {
                        Ok(out) if out.status.success() => Ok(()),
                        Ok(out) => {
                            let stderr = String::from_utf8_lossy(&out.stderr);
                            Err(format!("Fetch failed: {}", stderr))
                        }
                        Err(e) => Err(format!("Fetch error: {}", e)),
                    }
                } else {
                    Err("No repository".to_string())
                };

                // メインスレッドに戻ってUI更新
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_weak_clone.upgrade() {
                        match fetch_result {
                            Ok(()) => {
                                ui.set_status_message("Refresh & Fetch: Updating...".into());
                                ui.invoke_update_local_state();
                            }
                            Err(e) => {
                                ui.set_status_message(SharedString::from(e));
                                // エラーでもローカル状態は更新
                                ui.invoke_update_local_state();
                            }
                        }
                    }
                });
            });
        });
    }

    // Update local state (内部リフレッシュ用コールバック)
    {
        let refresh = refresh_ui.clone();
        let ui_weak = ui.as_weak();
        ui.on_update_local_state(move || {
            refresh();
            if let Some(ui) = ui_weak.upgrade() {
                // Fetchingメッセージをクリア（既にセットされていなければ）
                let current_msg = ui.get_status_message();
                if current_msg == "Refresh & Fetch: Updating..." {
                    ui.set_status_message("".into());
                }
            }
        });
    }

    // Stage file
    {
        let git_client = git_client.clone();
        let refresh = refresh_ui.clone();
        let ui_weak = ui.as_weak();
        ui.on_stage_file(move |filename| {
            let client = git_client.borrow();
            if let Err(e) = client.stage_file(&filename) {
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_status_message(SharedString::from(format!("Stage error: {}", e)));
                }
            }
            drop(client);
            refresh();
        });
    }

    // Stage all
    {
        let git_client = git_client.clone();
        let refresh = refresh_ui.clone();
        let ui_weak = ui.as_weak();
        ui.on_stage_all(move || {
            let client = git_client.borrow();
            if let Err(e) = client.stage_all() {
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_status_message(SharedString::from(format!("Stage all error: {}", e)));
                }
            }
            drop(client);
            refresh();
        });
    }

    // Unstage file
    {
        let git_client = git_client.clone();
        let refresh = refresh_ui.clone();
        let ui_weak = ui.as_weak();
        ui.on_unstage_file(move |filename| {
            let client = git_client.borrow();
            if let Err(e) = client.unstage_file(&filename) {
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_status_message(SharedString::from(format!("Unstage error: {}", e)));
                }
            }
            drop(client);
            refresh();
        });
    }

    // Discard file changes
    {
        let git_client = git_client.clone();
        let refresh = refresh_ui.clone();
        let ui_weak = ui.as_weak();
        ui.on_discard_file(move |filename| {
            let client = git_client.borrow();
            match client.discard_file(&filename) {
                Ok(()) => {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_status_message(SharedString::from(format!(
                            "Discarded changes: {}",
                            filename
                        )));
                    }
                }
                Err(e) => {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_status_message(SharedString::from(format!("Discard error: {}", e)));
                    }
                }
            }
            drop(client);
            refresh();
        });
    }

    // Unstage all
    {
        let git_client = git_client.clone();
        let refresh = refresh_ui.clone();
        let ui_weak = ui.as_weak();
        ui.on_unstage_all(move || {
            let client = git_client.borrow();
            if let Err(e) = client.unstage_all() {
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_status_message(SharedString::from(format!("Unstage all error: {}", e)));
                }
            }
            drop(client);
            refresh();
        });
    }

    // Toggle staged check
    {
        let ui_weak = ui.as_weak();
        ui.on_toggle_staged_check(move |idx, checked| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let checked_model = ui.get_staged_checked();
            let idx = idx as usize;
            if idx < checked_model.row_count() {
                checked_model.set_row_data(idx, checked);
                // カウント更新
                let count = (0..checked_model.row_count())
                    .filter(|&i| checked_model.row_data(i).unwrap_or(false))
                    .count();
                ui.set_staged_checked_count(count as i32);
            }
        });
    }

    // Toggle unstaged check
    {
        let ui_weak = ui.as_weak();
        ui.on_toggle_unstaged_check(move |idx, checked| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let checked_model = ui.get_unstaged_checked();
            let idx = idx as usize;
            if idx < checked_model.row_count() {
                checked_model.set_row_data(idx, checked);
                // カウント更新
                let count = (0..checked_model.row_count())
                    .filter(|&i| checked_model.row_data(i).unwrap_or(false))
                    .count();
                ui.set_unstaged_checked_count(count as i32);
            }
        });
    }

    // Staged range select (Shift+Click)
    {
        let ui_weak = ui.as_weak();
        ui.on_staged_range_select(move |idx| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let last = ui.get_last_clicked_staged();
            if last < 0 {
                // 前回クリックがない場合は単一選択
                ui.invoke_toggle_staged_check(idx, true);
                return;
            }
            let checked_model = ui.get_staged_checked();
            let start = last.min(idx) as usize;
            let end = last.max(idx) as usize;
            for i in start..=end {
                if i < checked_model.row_count() {
                    checked_model.set_row_data(i, true);
                }
            }
            let count = (0..checked_model.row_count())
                .filter(|&i| checked_model.row_data(i).unwrap_or(false))
                .count();
            ui.set_staged_checked_count(count as i32);
        });
    }

    // Unstaged range select (Shift+Click)
    {
        let ui_weak = ui.as_weak();
        ui.on_unstaged_range_select(move |idx| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let last = ui.get_last_clicked_unstaged();
            if last < 0 {
                ui.invoke_toggle_unstaged_check(idx, true);
                return;
            }
            let checked_model = ui.get_unstaged_checked();
            let start = last.min(idx) as usize;
            let end = last.max(idx) as usize;
            for i in start..=end {
                if i < checked_model.row_count() {
                    checked_model.set_row_data(i, true);
                }
            }
            let count = (0..checked_model.row_count())
                .filter(|&i| checked_model.row_data(i).unwrap_or(false))
                .count();
            ui.set_unstaged_checked_count(count as i32);
        });
    }

    // Stage selected files
    {
        let git_client = git_client.clone();
        let refresh = refresh_ui.clone();
        let ui_weak = ui.as_weak();
        ui.on_stage_selected(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let client = git_client.borrow();
            let files = ui.get_unstaged_files();
            let checked = ui.get_unstaged_checked();
            let mut staged_count = 0;

            for i in 0..files.row_count() {
                if let (Some(file), Some(is_checked)) = (files.row_data(i), checked.row_data(i)) {
                    if is_checked {
                        if client.stage_file(&file.filename).is_ok() {
                            staged_count += 1;
                        }
                    }
                }
            }
            drop(client);
            if staged_count > 0 {
                ui.set_status_message(SharedString::from(format!("Staged {} files", staged_count)));
            }
            refresh();
        });
    }

    // Unstage selected files
    {
        let git_client = git_client.clone();
        let refresh = refresh_ui.clone();
        let ui_weak = ui.as_weak();
        ui.on_unstage_selected(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let client = git_client.borrow();
            let files = ui.get_staged_files();
            let checked = ui.get_staged_checked();
            let mut unstaged_count = 0;

            for i in 0..files.row_count() {
                if let (Some(file), Some(is_checked)) = (files.row_data(i), checked.row_data(i)) {
                    if is_checked {
                        if client.unstage_file(&file.filename).is_ok() {
                            unstaged_count += 1;
                        }
                    }
                }
            }
            drop(client);
            if unstaged_count > 0 {
                ui.set_status_message(SharedString::from(format!(
                    "Unstaged {} files",
                    unstaged_count
                )));
            }
            refresh();
        });
    }

    // Discard selected files
    {
        let git_client = git_client.clone();
        let refresh = refresh_ui.clone();
        let ui_weak = ui.as_weak();
        ui.on_discard_selected(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let client = git_client.borrow();
            let files = ui.get_unstaged_files();
            let checked = ui.get_unstaged_checked();
            let mut discarded_count = 0;

            for i in 0..files.row_count() {
                if let (Some(file), Some(is_checked)) = (files.row_data(i), checked.row_data(i)) {
                    if is_checked {
                        if client.discard_file(&file.filename).is_ok() {
                            discarded_count += 1;
                        }
                    }
                }
            }
            drop(client);
            if discarded_count > 0 {
                ui.set_status_message(SharedString::from(format!(
                    "Discarded {} files",
                    discarded_count
                )));
            }
            refresh();
        });
    }

    // Commit
    {
        let git_client = git_client.clone();
        let refresh = refresh_ui.clone();
        let ui_weak = ui.as_weak();
        let history = commit_message_history.clone();
        ui.on_commit(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let message = ui.get_commit_message().to_string();
            if message.is_empty() {
                return;
            }
            let client = git_client.borrow();
            match client.commit(&message) {
                Ok(()) => {
                    // 履歴に追加
                    {
                        let mut hist = history.borrow_mut();
                        // 既に存在する場合は削除してから先頭に追加
                        hist.retain(|m| m != &message);
                        hist.insert(0, message.clone());
                        if hist.len() > MAX_COMMIT_HISTORY {
                            hist.truncate(MAX_COMMIT_HISTORY);
                        }
                        // UIに反映
                        let model: Vec<SharedString> = hist
                            .iter()
                            .map(|s| SharedString::from(s.as_str()))
                            .collect();
                        ui.set_commit_message_history(ModelRc::new(VecModel::from(model)));
                        // ファイルに保存
                        save_commit_history(&hist);
                    }
                    ui.set_commit_message("".into());
                    ui.set_commit_history_index(-1);
                    ui.set_status_message("Commit successful".into());
                }
                Err(e) => {
                    ui.set_status_message(SharedString::from(format!("Commit error: {}", e)));
                }
            }
            drop(client);
            refresh();
        });
    }

    // Commit and Push
    {
        let git_client = git_client.clone();
        let refresh = refresh_ui.clone();
        let ui_weak = ui.as_weak();
        let history = commit_message_history.clone();
        ui.on_commit_and_push(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let message = ui.get_commit_message().to_string();
            if message.is_empty() {
                return;
            }
            let client = git_client.borrow();
            match client.commit(&message) {
                Ok(()) => {
                    // 履歴に追加
                    {
                        let mut hist = history.borrow_mut();
                        hist.retain(|m| m != &message);
                        hist.insert(0, message.clone());
                        if hist.len() > MAX_COMMIT_HISTORY {
                            hist.truncate(MAX_COMMIT_HISTORY);
                        }
                        let model: Vec<SharedString> = hist
                            .iter()
                            .map(|s| SharedString::from(s.as_str()))
                            .collect();
                        ui.set_commit_message_history(ModelRc::new(VecModel::from(model)));
                        // ファイルに保存
                        save_commit_history(&hist);
                    }
                    ui.set_commit_message("".into());
                    ui.set_commit_history_index(-1);
                    // Pushを実行
                    match client.push() {
                        Ok(()) => {
                            ui.set_status_message("Commit & Push successful".into());
                        }
                        Err(e) => {
                            ui.set_status_message(SharedString::from(format!(
                                "Commit successful, but push failed: {}",
                                e
                            )));
                        }
                    }
                }
                Err(e) => {
                    ui.set_status_message(SharedString::from(format!("Commit error: {}", e)));
                }
            }
            drop(client);
            refresh();
        });
    }

    // Checkout branch
    {
        let git_client = git_client.clone();
        let refresh = refresh_ui.clone();
        let ui_weak = ui.as_weak();
        ui.on_checkout_branch(move |name| {
            let client = git_client.borrow();
            match client.checkout_branch(&name) {
                Ok(()) => {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_status_message(SharedString::from(format!("Switched to {}", name)));
                    }
                }
                Err(e) => {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_status_message(SharedString::from(format!("Checkout error: {}", e)));
                    }
                }
            }
            drop(client);
            refresh();
        });
    }

    // Create branch
    {
        let git_client = git_client.clone();
        let refresh = refresh_ui.clone();
        let ui_weak = ui.as_weak();
        ui.on_create_branch(move |name| {
            let client = git_client.borrow();
            match client.create_branch(&name) {
                Ok(()) => {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_status_message(SharedString::from(format!(
                            "Created branch: {}",
                            name
                        )));
                    }
                }
                Err(e) => {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_status_message(SharedString::from(format!(
                            "Create branch error: {}",
                            e
                        )));
                    }
                }
            }
            drop(client);
            refresh();
        });
    }

    // Delete branch
    {
        let git_client = git_client.clone();
        let refresh = refresh_ui.clone();
        let ui_weak = ui.as_weak();
        ui.on_delete_branch(move |name| {
            let client = git_client.borrow();
            match client.delete_branch(&name) {
                Ok(()) => {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_status_message(SharedString::from(format!(
                            "Deleted branch: {}",
                            name
                        )));
                    }
                }
                Err(e) => {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_status_message(SharedString::from(format!(
                            "Delete branch error: {}",
                            e
                        )));
                    }
                }
            }
            drop(client);
            refresh();
        });
    }

    // Merge branch
    {
        let git_client = git_client.clone();
        let refresh = refresh_ui.clone();
        let ui_weak = ui.as_weak();
        ui.on_merge_branch(move |name| {
            let client = git_client.borrow();
            match client.merge_branch(&name) {
                Ok(()) => {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_status_message(SharedString::from(format!("Merged: {}", name)));
                    }
                }
                Err(e) => {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_status_message(SharedString::from(format!("Merge error: {}", e)));
                    }
                }
            }
            drop(client);
            refresh();
        });
    }

    // Select commit
    {
        let git_client = git_client.clone();
        let ui_weak = ui.as_weak();
        ui.on_select_commit(move |_index, hash| {
            // 選択状態は既にSlint側で更新済み
            // まずDiffエリアをクリアして選択のフィードバックを即座に表示
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_diff_files(ModelRc::default());
                ui.set_diff_lines(ModelRc::default());
                ui.set_selected_diff_file(-1);
            }

            // リポジトリパスを取得
            let repo_path = {
                let client = git_client.borrow();
                client.get_repo_path()
            };

            let Some(repo_path) = repo_path else {
                return;
            };

            // 別スレッドでDiff計算を実行
            let ui_weak = ui_weak.clone();
            let hash = hash.to_string();
            std::thread::spawn(move || {
                let (diff_files, diff_lines, total_count) =
                    compute_commit_diff_in_thread(repo_path, hash.clone());

                // UIスレッドに結果を送信
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = ui_weak.upgrade() else {
                        return;
                    };
                    // 選択が変わっていないか確認
                    if ui.get_selected_commit_hash().to_string() != hash {
                        return;
                    }
                    ui.set_diff_files(Rc::new(slint::VecModel::from(diff_files)).into());
                    ui.set_selected_diff_file(-1);
                    ui.set_diff_lines(Rc::new(slint::VecModel::from(diff_lines)).into());
                    ui.set_diff_total_lines(total_count as i32);
                });
            });
        });
    }

    // Select diff file
    {
        let git_client = git_client.clone();
        let ui_weak = ui.as_weak();
        ui.on_select_diff_file(move |file_index| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let commit_hash = ui.get_selected_commit_hash().to_string();
            if commit_hash.is_empty() {
                return;
            }
            let client = git_client.borrow();
            let (diff_lines, total_count) =
                client.get_commit_file_diff(&commit_hash, file_index as usize);
            ui.set_diff_lines(Rc::new(slint::VecModel::from(diff_lines)).into());
            ui.set_diff_total_lines(total_count as i32);
        });
    }

    // Select file
    {
        let git_client = git_client.clone();
        let ui_weak = ui.as_weak();
        ui.on_select_file(move |filename, staged| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let client = git_client.borrow();
            let (diff_lines, total_count) = client.get_file_diff(&filename, staged);
            ui.set_diff_lines(Rc::new(slint::VecModel::from(diff_lines)).into());
            ui.set_diff_total_lines(total_count as i32);
            // Stage Hunk用にファイル情報を保存
            ui.set_current_diff_filename(filename.clone());
            ui.set_current_diff_is_staged(staged);
        });
    }

    // Stage hunk
    {
        let git_client = git_client.clone();
        let refresh = refresh_ui.clone();
        let ui_weak = ui.as_weak();
        ui.on_stage_hunk(move |hunk_index| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let filename = ui.get_current_diff_filename().to_string();
            if filename.is_empty() {
                ui.set_status_message("No file selected".into());
                return;
            }
            let client = git_client.borrow();
            match client.stage_hunk(&filename, hunk_index as usize) {
                Ok(()) => {
                    ui.set_status_message(SharedString::from(format!(
                        "Staged hunk {} of {}",
                        hunk_index + 1,
                        filename
                    )));
                    // Diffを更新
                    let (diff_lines, total_count) = client.get_file_diff(&filename, false);
                    ui.set_diff_lines(Rc::new(slint::VecModel::from(diff_lines)).into());
                    ui.set_diff_total_lines(total_count as i32);
                }
                Err(e) => {
                    ui.set_status_message(SharedString::from(format!("Stage hunk error: {}", e)));
                }
            }
            drop(client);
            refresh();
        });
    }

    // Checkout remote branch
    {
        let git_client = git_client.clone();
        let refresh = refresh_ui.clone();
        let ui_weak = ui.as_weak();
        ui.on_checkout_remote_branch(move |name| {
            let client = git_client.borrow();
            match client.checkout_remote_branch(&name) {
                Ok(()) => {
                    let local_name = name.split('/').skip(1).collect::<Vec<_>>().join("/");
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_status_message(SharedString::from(format!(
                            "Checked out {} from {}",
                            local_name, name
                        )));
                    }
                }
                Err(e) => {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_status_message(SharedString::from(format!("Checkout error: {}", e)));
                    }
                }
            }
            drop(client);
            refresh();
        });
    }

    // Pull/Push/Fetch
    {
        let git_client = git_client.clone();
        let refresh = refresh_ui.clone();
        let ui_weak = ui.as_weak();
        ui.on_pull(move || {
            let client = git_client.borrow();
            match client.pull() {
                Ok(()) => {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_status_message("Pull successful".into());
                    }
                    drop(client);
                    refresh();
                }
                Err(e) => {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_status_message(SharedString::from(format!("Pull error: {}", e)));
                    }
                    drop(client);
                    refresh();
                }
            }
        });
    }
    {
        let git_client = git_client.clone();
        let refresh = refresh_ui.clone();
        let ui_weak = ui.as_weak();
        ui.on_push(move || {
            let client = git_client.borrow();
            match client.push() {
                Ok(()) => {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_status_message("Push successful".into());
                    }
                    drop(client);
                    refresh();
                }
                Err(e) => {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_status_message(SharedString::from(format!("Push error: {}", e)));
                    }
                    drop(client);
                    refresh();
                }
            }
        });
    }

    // Copy commit hash to clipboard
    {
        let ui_weak = ui.as_weak();
        ui.on_copy_commit_hash(move |hash| {
            let hash_str = hash.to_string();
            copy_to_clipboard_async(hash_str.clone());
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_status_message(SharedString::from(format!(
                    "Copied: {}",
                    &hash_str[..7.min(hash_str.len())]
                )));
            }
        });
    }

    // Copy commit message to clipboard
    {
        let ui_weak = ui.as_weak();
        ui.on_copy_commit_message(move |message| {
            let message_str = message.to_string();
            copy_to_clipboard_async(message_str.clone());
            if let Some(ui) = ui_weak.upgrade() {
                // 長いメッセージは省略して表示（文字数ベースで安全にスライス）
                let display_msg: String = if message_str.chars().count() > 30 {
                    format!("{}...", message_str.chars().take(30).collect::<String>())
                } else {
                    message_str
                };
                ui.set_status_message(SharedString::from(format!("Copied: {}", display_msg)));
            }
        });
    }

    // Reset to commit
    {
        let git_client = git_client.clone();
        let refresh = refresh_ui.clone();
        let ui_weak = ui.as_weak();
        ui.on_reset_to_commit(move |index, mode| {
            let client = git_client.borrow();
            if let Some(hash) = client.get_commit_hash_by_index(index as usize) {
                match client.reset_to_commit(&hash, &mode) {
                    Ok(()) => {
                        if let Some(ui) = ui_weak.upgrade() {
                            ui.set_status_message(SharedString::from(format!(
                                "Reset ({}) to {}",
                                mode,
                                &hash[..7]
                            )));
                        }
                    }
                    Err(e) => {
                        if let Some(ui) = ui_weak.upgrade() {
                            ui.set_status_message(SharedString::from(format!(
                                "Reset error: {}",
                                e
                            )));
                        }
                    }
                }
            }
            drop(client);
            refresh();
        });
    }

    // Revert commit
    {
        let git_client = git_client.clone();
        let refresh = refresh_ui.clone();
        let ui_weak = ui.as_weak();
        ui.on_revert_commit(move |index| {
            let client = git_client.borrow();
            if let Some(hash) = client.get_commit_hash_by_index(index as usize) {
                match client.revert_commit(&hash) {
                    Ok(()) => {
                        if let Some(ui) = ui_weak.upgrade() {
                            ui.set_status_message(SharedString::from(format!(
                                "Reverted {}",
                                &hash[..7]
                            )));
                        }
                    }
                    Err(e) => {
                        if let Some(ui) = ui_weak.upgrade() {
                            ui.set_status_message(SharedString::from(format!(
                                "Revert error: {}",
                                e
                            )));
                        }
                    }
                }
            }
            drop(client);
            refresh();
        });
    }

    // Open commit on GitHub
    {
        let git_client = git_client.clone();
        let ui_weak = ui.as_weak();
        ui.on_open_commit_on_github(move |hash| {
            let client = git_client.borrow();
            if let Some(url) = client.get_commit_github_url(&hash) {
                if open::that(&url).is_ok() {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_status_message(SharedString::from(format!(
                            "Opening commit {}",
                            &hash[..7]
                        )));
                    }
                } else {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_status_message("Failed to open browser".into());
                    }
                }
            } else {
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_status_message("Not a GitHub repository".into());
                }
            }
        });
    }

    // Copy branch name to clipboard
    {
        let ui_weak = ui.as_weak();
        ui.on_copy_branch_name(move |name| {
            let name_str = name.to_string();
            copy_to_clipboard_async(name_str.clone());
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_status_message(SharedString::from(format!("Copied: {}", name_str)));
            }
        });
    }

    // Create Pull Request (open in browser)
    {
        let git_client = git_client.clone();
        let ui_weak = ui.as_weak();
        ui.on_create_pull_request(move |branch_name| {
            let client = git_client.borrow();
            if let Some(pr_url) = client.get_pull_request_url(&branch_name) {
                if open::that(&pr_url).is_ok() {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_status_message(SharedString::from(format!(
                            "Opening PR for {}",
                            branch_name
                        )));
                    }
                } else {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_status_message("Failed to open browser".into());
                    }
                }
            } else {
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_status_message("Not a GitHub repository".into());
                }
            }
        });
    }

    // Navigate commit message history (keyboard up/down)
    {
        let history = commit_message_history.clone();
        let ui_weak = ui.as_weak();
        ui.on_navigate_commit_history(move |direction| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let hist = history.borrow();
            if hist.is_empty() {
                return;
            }

            let current_index = ui.get_commit_history_index();
            let new_index = if direction > 0 {
                // Up: 履歴を遡る
                let next = current_index + 1;
                if next < hist.len() as i32 {
                    next
                } else {
                    current_index
                }
            } else {
                // Down: 履歴を進む
                if current_index > 0 {
                    current_index - 1
                } else if current_index == 0 {
                    -1 // 最新（空の状態）に戻る
                } else {
                    current_index
                }
            };

            ui.set_commit_history_index(new_index);
            if new_index >= 0 && (new_index as usize) < hist.len() {
                ui.set_commit_message(SharedString::from(hist[new_index as usize].as_str()));
            } else {
                ui.set_commit_message("".into());
            }
        });
    }

    // Stash operations
    {
        let git_client = git_client.clone();
        let refresh = refresh_ui.clone();
        let ui_weak = ui.as_weak();
        ui.on_stash_save(move |message, include_untracked| {
            let mut client = git_client.borrow_mut();
            match client.stash_save(&message, include_untracked) {
                Ok(_) => {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_status_message("Stash saved".into());
                    }
                }
                Err(e) => {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_status_message(SharedString::from(format!(
                            "Stash save error: {}",
                            e
                        )));
                    }
                }
            }
            drop(client);
            refresh();
        });
    }
    {
        let git_client = git_client.clone();
        let refresh = refresh_ui.clone();
        let ui_weak = ui.as_weak();
        ui.on_stash_apply(move |index| {
            let mut client = git_client.borrow_mut();
            // applyは競合する可能性があるため、エラー時はリロードして状態を更新
            match client.stash_apply(index as usize) {
                Ok(_) => {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_status_message("Stash applied".into());
                    }
                }
                Err(e) => {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_status_message(SharedString::from(format!(
                            "Stash apply error: {}",
                            e
                        )));
                    }
                }
            }
            drop(client);
            refresh();
        });
    }
    {
        let git_client = git_client.clone();
        let refresh = refresh_ui.clone();
        let ui_weak = ui.as_weak();
        ui.on_stash_pop(move |index| {
            let mut client = git_client.borrow_mut();
            match client.stash_pop(index as usize) {
                Ok(_) => {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_status_message("Stash popped".into());
                    }
                }
                Err(e) => {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_status_message(SharedString::from(format!(
                            "Stash pop error: {}",
                            e
                        )));
                    }
                }
            }
            drop(client);
            refresh();
        });
    }
    {
        let git_client = git_client.clone();
        let refresh = refresh_ui.clone();
        let ui_weak = ui.as_weak();
        ui.on_stash_drop(move |index| {
            let mut client = git_client.borrow_mut();
            match client.stash_drop(index as usize) {
                Ok(_) => {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_status_message("Stash dropped".into());
                    }
                }
                Err(e) => {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_status_message(SharedString::from(format!(
                            "Stash drop error: {}",
                            e
                        )));
                    }
                }
            }
            drop(client);
            refresh();
        });
    }

    // 起動時に最初のリポジトリを自動で開く
    if let Some(repo_path) = initial_repo {
        let mut client = git_client.borrow_mut();
        if client.open_repo(&repo_path).is_ok() {
            drop(client);

            // UIにリポジトリ名を設定
            let repo_name = Path::new(&repo_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&repo_path)
                .to_string();
            ui.set_repo_name(SharedString::from(repo_name));

            refresh_ui();
        }
    }

    ui.run()
}

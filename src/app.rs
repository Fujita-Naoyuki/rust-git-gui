//! アプリケーションの状態と操作

use std::path::Path;
use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::component::VirtualListScrollHandle;
use gpui_kit::*;

use crate::config::{self, RepoEntry, RepoListItem};
use crate::git::{self, GitClient, RepoSnapshot};
use crate::graph::{self, GraphShape};
use crate::model::*;

/// コミットメッセージ履歴の最大保持数
const MAX_COMMIT_HISTORY: usize = 10;
/// グラフ列の最小幅
const MIN_GRAPH_WIDTH: f32 = 48.0;

/// リポジトリ一覧のドロップ位置
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropZone {
    /// 上端（前に挿入）
    Before,
    /// 中央（グループに追加）
    Center,
    /// 下端（後に挿入）
    After,
}

/// ドラッグ中のリポジトリ
#[derive(Debug, Clone)]
pub struct DraggedRepo {
    pub item_index: usize,
    pub name: String,
}

/// コミットモードでのファイル選択
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileSelection {
    pub staged: bool,
    pub index: usize,
}

/// Stage/Unstageリストの複数選択状態
#[derive(Debug, Clone, Default)]
pub struct CheckState {
    pub checked: Vec<bool>,
    pub last_clicked: Option<usize>,
}

impl CheckState {
    fn reset(&mut self, len: usize) {
        self.checked = vec![false; len];
        self.last_clicked = None;
    }

    pub fn count(&self) -> usize {
        self.checked.iter().filter(|c| **c).count()
    }

    pub fn is_checked(&self, idx: usize) -> bool {
        self.checked.get(idx).copied().unwrap_or(false)
    }

    pub fn set(&mut self, idx: usize, value: bool) {
        if let Some(c) = self.checked.get_mut(idx) {
            *c = value;
        }
        self.last_clicked = Some(idx);
    }

    /// Shift+Click: 前回クリック位置からの範囲を選択
    pub fn select_range(&mut self, idx: usize) {
        let Some(last) = self.last_clicked else {
            self.set(idx, true);
            return;
        };
        for i in last.min(idx)..=last.max(idx) {
            if let Some(c) = self.checked.get_mut(i) {
                *c = true;
            }
        }
    }
}

pub struct GitApp {
    pub(crate) client: GitClient,
    pub(crate) repo_path: Option<String>,
    pub(crate) repo_name: String,
    pub(crate) snapshot: RepoSnapshot,
    pub(crate) status_message: String,
    pub(crate) loading: bool,

    // 通常モード
    pub(crate) selected_commit: Option<usize>,
    pub(crate) selected_commit_hash: String,
    pub(crate) diff_files: Vec<DiffFileData>,
    pub(crate) selected_diff_file: Option<usize>,
    pub(crate) diff: DiffContent,
    pub(crate) commit_scroll: VirtualListScrollHandle,
    /// コミット一覧の各行サイズ（仮想リスト用）
    pub(crate) commit_item_sizes: Rc<Vec<Size<Pixels>>>,
    /// グラフ列の幅
    pub(crate) graph_width: f32,

    // サイドバー
    pub(crate) selected_branch: Option<usize>,
    pub(crate) selected_remote_branch: Option<usize>,

    // コミットモード
    pub(crate) commit_mode: bool,
    pub(crate) selected_file: Option<FileSelection>,
    /// 表示中のワーキングツリーDiff（ファイル名, staged）。Stage Hunk用
    pub(crate) current_diff_file: Option<(String, bool)>,
    pub(crate) staged_check: CheckState,
    pub(crate) unstaged_check: CheckState,
    pub(crate) commit_input: Entity<TextareaState>,
    pub(crate) commit_history: Vec<String>,
    pub(crate) commit_history_index: Option<usize>,

    // リポジトリ一覧
    pub(crate) show_repo_sidebar: bool,
    pub(crate) repo_entries: Vec<RepoEntry>,
    pub(crate) repo_items: Vec<RepoListItem>,
    pub(crate) drag_target: Option<(usize, DropZone)>,

    refresh_task: Option<Task<()>>,
    diff_task: Option<Task<()>>,
    remote_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl GitApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let commit_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(3)
                .placeholder("Commit message... (↑ for history)\n\nLine 2 for details (optional)")
        });
        let subscriptions = vec![
            cx.subscribe(&commit_input, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    // 手入力されたら履歴ナビゲーションを解除
                    if this.commit_history_index.is_some() && this.commit_message(cx).is_empty() {
                        this.commit_history_index = None;
                    }
                    cx.notify();
                }
            }),
        ];

        let repo_entries = config::load_repo_entries();
        let mut this = Self {
            client: GitClient::new(),
            repo_path: None,
            repo_name: String::new(),
            snapshot: RepoSnapshot::default(),
            status_message: String::new(),
            loading: false,
            selected_commit: None,
            selected_commit_hash: String::new(),
            diff_files: vec![],
            selected_diff_file: None,
            diff: DiffContent::default(),
            commit_scroll: VirtualListScrollHandle::new(),
            commit_item_sizes: Rc::new(vec![]),
            graph_width: MIN_GRAPH_WIDTH,
            selected_branch: None,
            selected_remote_branch: None,
            commit_mode: false,
            selected_file: None,
            current_diff_file: None,
            staged_check: CheckState::default(),
            unstaged_check: CheckState::default(),
            commit_input,
            commit_history: config::load_commit_history(),
            commit_history_index: None,
            show_repo_sidebar: false,
            repo_items: config::flatten_repo_entries(&repo_entries),
            repo_entries,
            drag_target: None,
            refresh_task: None,
            diff_task: None,
            remote_task: None,
            _subscriptions: subscriptions,
        };

        // 起動時に最初のリポジトリを自動で開く
        if let Some(first) = config::all_repo_paths(&this.repo_entries).first().cloned() {
            this.open_repo(&first, cx);
        }
        this
    }

    // ========== 共通 ==========

    pub(crate) fn set_status(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.status_message = message.into();
        cx.notify();
    }

    /// Result をステータスメッセージに反映する
    fn report(
        &mut self,
        result: Result<(), String>,
        ok: impl Into<String>,
        err_label: &str,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(()) => self.set_status(ok, cx),
            Err(e) => self.set_status(format!("{}: {}", err_label, e), cx),
        }
    }

    pub(crate) fn commit_message(&self, cx: &App) -> String {
        self.commit_input.read(cx).value().to_string()
    }

    fn set_commit_message(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_input.update(cx, |input, cx| {
            input.set_value(text.to_string(), window, cx)
        });
    }

    // ========== リポジトリ ==========

    pub(crate) fn open_repo(&mut self, path: &str, cx: &mut Context<Self>) {
        match self.client.open_repo(path) {
            Ok(()) => {
                self.repo_path = Some(path.to_string());
                self.repo_name = Path::new(path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(path)
                    .to_string();
                self.set_repo_entries(config::add_recent_repo(path));
                self.snapshot = RepoSnapshot::default();
                self.clear_diff();
                self.set_status("Repository opened", cx);
                self.refresh(cx);
            }
            Err(e) => self.set_status(format!("Error: {}", e), cx),
        }
    }

    pub(crate) fn set_repo_entries(&mut self, entries: Vec<RepoEntry>) {
        self.repo_items = config::flatten_repo_entries(&entries);
        self.repo_entries = entries;
    }

    pub(crate) fn update_repo_entries(&mut self, entries: Vec<RepoEntry>, cx: &mut Context<Self>) {
        self.set_repo_entries(entries);
        cx.notify();
    }

    pub(crate) fn browse_repo(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Select Git Repository".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let path = path.to_string_lossy().to_string();
            this.update(cx, |this, cx| this.open_repo(&path, cx)).ok();
        })
        .detach();
    }

    /// クローン先フォルダを選択して入力欄に設定する
    pub(crate) fn browse_clone_path(&mut self, input: Entity<InputState>, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Select Destination Folder".into()),
        });
        cx.spawn(async move |_, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let path = path.to_string_lossy().to_string();
            cx.update_window_entity_input(input, path);
        })
        .detach();
    }

    /// クローンを実行する。結果は `on_done` に渡される（エラー時はメッセージ）
    pub(crate) fn clone_repo(
        &mut self,
        url: String,
        dest: String,
        on_done: impl FnOnce(Result<(), String>, &mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_status("Cloning...", cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { git::clone_repo(&url, &dest) })
                .await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(path) => {
                    this.set_status("Clone successful", cx);
                    this.open_repo(&path, cx);
                    on_done(Ok(()), window, cx);
                }
                Err(e) => {
                    this.set_status("", cx);
                    on_done(Err(e), window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    // ========== 状態の再読み込み ==========

    /// ローカルの状態をバックグラウンドで再読み込みする
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.repo_path.clone() else {
            return;
        };
        self.loading = true;
        cx.notify();
        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { git::load_snapshot(&path) })
                .await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(snapshot) => this.apply_snapshot(snapshot),
                    Err(e) => this.status_message = format!("Error: {}", e),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn apply_snapshot(&mut self, snapshot: RepoSnapshot) {
        self.staged_check.reset(snapshot.staged.len());
        self.unstaged_check.reset(snapshot.unstaged.len());
        self.commit_item_sizes = Rc::new(vec![
            size(px(0.), px(graph::ROW_HEIGHT));
            snapshot.commits.len()
        ]);
        // 最も右にあるレーンに合わせてグラフ列の幅を決める
        let max_x = snapshot
            .commits
            .iter()
            .flat_map(|c| {
                c.graph
                    .segments
                    .iter()
                    .map(|s| match s.shape {
                        GraphShape::Line { from, to } => from.0.max(to.0),
                        GraphShape::Curve { from, to, .. } => from.0.max(to.0),
                    })
                    .chain(std::iter::once(c.graph.node_x))
            })
            .fold(0.0f32, f32::max);
        self.graph_width = (max_x + graph::COL_SPACING).max(MIN_GRAPH_WIDTH);
        self.snapshot = snapshot;
        self.selected_branch = None;
        self.selected_remote_branch = None;
        self.clear_diff();
    }

    fn clear_diff(&mut self) {
        self.selected_commit = None;
        self.selected_commit_hash.clear();
        self.selected_file = None;
        self.selected_diff_file = None;
        self.current_diff_file = None;
        self.diff_files.clear();
        self.diff = DiffContent::default();
        self.diff_task = None;
    }

    /// Fetchしてから状態を再読み込みする
    pub(crate) fn fetch_and_refresh(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.repo_path.clone() else {
            return;
        };
        self.set_status("Refresh & Fetch: Fetching...", cx);
        self.run_remote(
            move || git::fetch_all(&path),
            |this, result, _| {
                // エラーでもローカル状態は更新
                this.status_message = match result {
                    Ok(()) => String::new(),
                    Err(e) => e,
                };
            },
            cx,
        );
    }

    pub(crate) fn pull(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.repo_path.clone() else {
            return;
        };
        self.set_status("Pulling...", cx);
        self.run_remote(
            move || git::pull_repo(&path),
            |this, result, _| {
                this.status_message = match result {
                    Ok(()) => "Pull successful".into(),
                    Err(e) => format!("Pull error: {}", e),
                };
            },
            cx,
        );
    }

    pub(crate) fn push(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.repo_path.clone() else {
            return;
        };
        self.set_status("Pushing...", cx);
        self.run_remote(
            move || git::push_repo(&path),
            |this, result, _| {
                this.status_message = match result {
                    Ok(()) => "Push successful".into(),
                    Err(e) => format!("Push error: {}", e),
                };
            },
            cx,
        );
    }

    /// ネットワークを伴うgit操作をバックグラウンドで実行し、完了後に再読み込みする
    fn run_remote(
        &mut self,
        work: impl FnOnce() -> Result<(), String> + Send + 'static,
        done: impl FnOnce(&mut Self, Result<(), String>, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.remote_task = Some(cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { work() }).await;
            this.update(cx, |this, cx| {
                done(this, result, cx);
                this.refresh(cx);
            })
            .ok();
        }));
    }

    pub(crate) fn is_remote_busy(&self) -> bool {
        self.remote_task.is_some() && self.status_message.ends_with("...")
    }

    // ========== コミット選択・Diff ==========

    pub(crate) fn select_commit(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(commit) = self.snapshot.commits.get(index) else {
            return;
        };
        let hash = commit.full_hash.clone();
        self.selected_commit = Some(index);
        self.selected_commit_hash = hash.clone();
        self.diff_files.clear();
        self.diff = DiffContent::default();
        self.selected_diff_file = None;
        cx.notify();

        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
        if hash.is_empty() {
            return;
        }

        // 別スレッドでDiff計算を実行
        self.diff_task = Some(cx.spawn(async move |this, cx| {
            let task_hash = hash.clone();
            let (files, content) = cx
                .background_executor()
                .spawn(async move { git::compute_commit_diff(&repo_path, &task_hash) })
                .await;
            this.update(cx, |this, cx| {
                // 選択が変わっていないか確認
                if this.selected_commit_hash != hash {
                    return;
                }
                this.diff_files = files;
                this.diff = content;
                this.selected_diff_file = (!this.diff_files.is_empty()).then_some(0);
                cx.notify();
            })
            .ok();
        }));
    }

    pub(crate) fn select_diff_file(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.selected_commit_hash.is_empty() {
            return;
        }
        self.selected_diff_file = Some(index);
        self.diff = self
            .client
            .get_commit_file_diff(&self.selected_commit_hash, index);
        cx.notify();
    }

    pub(crate) fn enter_commit_mode(&mut self, cx: &mut Context<Self>) {
        self.commit_mode = true;
        self.clear_diff();
        cx.notify();
    }

    pub(crate) fn exit_commit_mode(&mut self, cx: &mut Context<Self>) {
        self.commit_mode = false;
        self.clear_diff();
        cx.notify();
    }

    // ========== Stage / Unstage ==========

    pub(crate) fn files(&self, staged: bool) -> &[FileData] {
        if staged {
            &self.snapshot.staged
        } else {
            &self.snapshot.unstaged
        }
    }

    pub(crate) fn check_state_mut(&mut self, staged: bool) -> &mut CheckState {
        if staged {
            &mut self.staged_check
        } else {
            &mut self.unstaged_check
        }
    }

    /// ファイルをクリックしてDiffを表示する
    pub(crate) fn select_file(&mut self, staged: bool, index: usize, cx: &mut Context<Self>) {
        let Some(file) = self.files(staged).get(index) else {
            return;
        };
        let filename = file.filename.clone();
        self.selected_file = Some(FileSelection { staged, index });
        self.diff = self.client.get_file_diff(&filename, staged);
        self.current_diff_file = Some((filename, staged));
        self.check_state_mut(staged).set(index, true);
        cx.notify();
    }

    pub(crate) fn toggle_check(&mut self, staged: bool, index: usize, cx: &mut Context<Self>) {
        let state = self.check_state_mut(staged);
        let value = !state.is_checked(index);
        state.set(index, value);
        cx.notify();
    }

    pub(crate) fn range_select(&mut self, staged: bool, index: usize, cx: &mut Context<Self>) {
        self.check_state_mut(staged).select_range(index);
        cx.notify();
    }

    pub(crate) fn stage_file(&mut self, filename: &str, cx: &mut Context<Self>) {
        if let Err(e) = self.client.stage_file(filename) {
            self.set_status(format!("Stage error: {}", e), cx);
        }
        self.refresh(cx);
    }

    pub(crate) fn unstage_file(&mut self, filename: &str, cx: &mut Context<Self>) {
        if let Err(e) = self.client.unstage_file(filename) {
            self.set_status(format!("Unstage error: {}", e), cx);
        }
        self.refresh(cx);
    }

    pub(crate) fn stage_all(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = self.client.stage_all() {
            self.set_status(format!("Stage all error: {}", e), cx);
        }
        self.refresh(cx);
    }

    pub(crate) fn unstage_all(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = self.client.unstage_all() {
            self.set_status(format!("Unstage all error: {}", e), cx);
        }
        self.refresh(cx);
    }

    fn checked_files(&self, staged: bool) -> Vec<String> {
        let state = if staged {
            &self.staged_check
        } else {
            &self.unstaged_check
        };
        self.files(staged)
            .iter()
            .enumerate()
            .filter(|(i, _)| state.is_checked(*i))
            .map(|(_, f)| f.filename.clone())
            .collect()
    }

    pub(crate) fn stage_selected(&mut self, cx: &mut Context<Self>) {
        let count = self
            .checked_files(false)
            .iter()
            .filter(|f| self.client.stage_file(f).is_ok())
            .count();
        if count > 0 {
            self.set_status(format!("Staged {} files", count), cx);
        }
        self.refresh(cx);
    }

    pub(crate) fn unstage_selected(&mut self, cx: &mut Context<Self>) {
        let count = self
            .checked_files(true)
            .iter()
            .filter(|f| self.client.unstage_file(f).is_ok())
            .count();
        if count > 0 {
            self.set_status(format!("Unstaged {} files", count), cx);
        }
        self.refresh(cx);
    }

    pub(crate) fn discard_selected(&mut self, cx: &mut Context<Self>) {
        let count = self
            .checked_files(false)
            .iter()
            .filter(|f| self.client.discard_file(f).is_ok())
            .count();
        if count > 0 {
            self.set_status(format!("Discarded {} files", count), cx);
        }
        self.refresh(cx);
    }

    pub(crate) fn stage_hunk(&mut self, hunk_index: usize, cx: &mut Context<Self>) {
        let Some((filename, false)) = self.current_diff_file.clone() else {
            self.set_status("No file selected", cx);
            return;
        };
        match self.client.stage_hunk(&filename, hunk_index) {
            Ok(()) => {
                self.set_status(
                    format!("Staged hunk {} of {}", hunk_index + 1, filename),
                    cx,
                );
                self.diff = self.client.get_file_diff(&filename, false);
            }
            Err(e) => self.set_status(format!("Stage hunk error: {}", e), cx),
        }
        // Diff表示は維持したまま一覧だけ更新する
        let Some(path) = self.repo_path.clone() else {
            return;
        };
        let current = self.current_diff_file.clone();
        let diff = self.diff.clone();
        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { git::load_snapshot(&path) })
                .await;
            this.update(cx, |this, cx| {
                if let Ok(snapshot) = result {
                    this.apply_snapshot(snapshot);
                    this.current_diff_file = current;
                    this.diff = diff;
                }
                cx.notify();
            })
            .ok();
        }));
    }

    // ========== コミット ==========

    fn push_commit_history(&mut self, message: &str) {
        self.commit_history.retain(|m| m != message);
        self.commit_history.insert(0, message.to_string());
        self.commit_history.truncate(MAX_COMMIT_HISTORY);
        config::save_commit_history(&self.commit_history);
    }

    pub(crate) fn can_commit(&self, cx: &App) -> bool {
        !self.snapshot.staged.is_empty() && !self.commit_message(cx).trim().is_empty()
    }

    pub(crate) fn commit(&mut self, and_push: bool, window: &mut Window, cx: &mut Context<Self>) {
        let message = self.commit_message(cx);
        if message.trim().is_empty() {
            return;
        }
        match self.client.commit(&message) {
            Ok(()) => {
                self.push_commit_history(&message);
                self.commit_history_index = None;
                self.set_commit_message("", window, cx);
                self.commit_mode = false;
                if and_push {
                    self.push(cx);
                    self.status_message = "Committed. Pushing...".into();
                } else {
                    self.set_status("Commit successful", cx);
                    self.refresh(cx);
                }
            }
            Err(e) => {
                self.set_status(format!("Commit error: {}", e), cx);
                self.refresh(cx);
            }
        }
    }

    /// ↑↓キーによるコミットメッセージ履歴のナビゲーション。処理した場合はtrue
    pub(crate) fn navigate_commit_history(
        &mut self,
        up: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.commit_history.is_empty() {
            return false;
        }
        // 履歴ナビゲーション中か、入力が空のときのみ
        let active = self.commit_history_index.is_some();
        if !active && !(up && self.commit_message(cx).is_empty()) {
            return false;
        }

        let new_index = match (up, self.commit_history_index) {
            (true, None) => Some(0),
            (true, Some(i)) => Some((i + 1).min(self.commit_history.len() - 1)),
            (false, Some(0)) | (false, None) => None,
            (false, Some(i)) => Some(i - 1),
        };
        self.commit_history_index = new_index;
        let text = new_index
            .map(|i| self.commit_history[i].clone())
            .unwrap_or_default();
        self.set_commit_message(&text, window, cx);
        // set_valueの後にChangeイベントが来ても解除されないよう再設定
        self.commit_history_index = new_index;
        cx.notify();
        true
    }

    pub(crate) fn use_commit_history(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(text) = self.commit_history.get(index).cloned() {
            self.set_commit_message(&text, window, cx);
            self.commit_history_index = Some(index);
            cx.notify();
        }
    }

    // ========== ブランチ ==========

    pub(crate) fn checkout_branch(&mut self, name: &str, cx: &mut Context<Self>) {
        let result = self.client.checkout_branch(name);
        self.report(
            result,
            format!("Switched to {}", name),
            "Checkout error",
            cx,
        );
        self.refresh(cx);
    }

    pub(crate) fn checkout_remote_branch(&mut self, name: &str, cx: &mut Context<Self>) {
        let local_name = name.split('/').skip(1).collect::<Vec<_>>().join("/");
        let result = self.client.checkout_remote_branch(name);
        self.report(
            result,
            format!("Checked out {} from {}", local_name, name),
            "Checkout error",
            cx,
        );
        self.refresh(cx);
    }

    pub(crate) fn create_branch(
        &mut self,
        name: &str,
        at_commit: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let result = match at_commit {
            Some(hash) => self.client.create_branch_at_commit(name, hash),
            None => self.client.create_branch(name),
        };
        let ok = match at_commit {
            Some(hash) => format!("Created branch '{}' at {}", name, short_hash(hash)),
            None => format!("Created branch: {}", name),
        };
        self.report(result, ok, "Create branch error", cx);
        self.refresh(cx);
    }

    pub(crate) fn delete_branch(&mut self, name: &str, cx: &mut Context<Self>) {
        let result = self.client.delete_branch(name);
        self.report(
            result,
            format!("Deleted branch: {}", name),
            "Delete branch error",
            cx,
        );
        self.refresh(cx);
    }

    pub(crate) fn merge_branch(&mut self, name: &str, cx: &mut Context<Self>) {
        let result = self.client.merge_branch(name);
        self.report(result, format!("Merged: {}", name), "Merge error", cx);
        self.refresh(cx);
    }

    pub(crate) fn create_pull_request(&mut self, branch: &str, cx: &mut Context<Self>) {
        match self.client.get_pull_request_url(branch) {
            Some(url) => {
                cx.open_url(&url);
                self.set_status(format!("Opening PR for {}", branch), cx);
            }
            None => self.set_status("Not a GitHub repository", cx),
        }
    }

    pub(crate) fn copy_text(&mut self, text: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
        // 長いテキストは省略して表示（文字数ベースで安全にスライス）
        let display: String = if text.chars().count() > 30 {
            format!("{}...", text.chars().take(30).collect::<String>())
        } else {
            text.to_string()
        };
        self.set_status(format!("Copied: {}", display), cx);
    }

    // ========== コミット操作 ==========

    pub(crate) fn open_commit_on_github(&mut self, hash: &str, cx: &mut Context<Self>) {
        match self.client.get_commit_github_url(hash) {
            Some(url) => {
                cx.open_url(&url);
                self.set_status(format!("Opening commit {}", short_hash(hash)), cx);
            }
            None => self.set_status("Not a GitHub repository", cx),
        }
    }

    pub(crate) fn reset_to_commit(&mut self, hash: &str, mode: &str, cx: &mut Context<Self>) {
        let result = self.client.reset_to_commit(hash, mode);
        self.report(
            result,
            format!("Reset ({}) to {}", mode, short_hash(hash)),
            "Reset error",
            cx,
        );
        self.refresh(cx);
    }

    pub(crate) fn revert_commit(&mut self, hash: &str, cx: &mut Context<Self>) {
        let result = self.client.revert_commit(hash);
        self.report(
            result,
            format!("Reverted {}", short_hash(hash)),
            "Revert error",
            cx,
        );
        self.refresh(cx);
    }

    // ========== Stash ==========

    pub(crate) fn stash_save(&mut self, message: &str, cx: &mut Context<Self>) {
        let result = self.client.stash_save(message, true);
        self.report(result, "Stash saved", "Stash save error", cx);
        self.refresh(cx);
    }

    pub(crate) fn stash_apply(&mut self, index: usize, cx: &mut Context<Self>) {
        // applyは競合する可能性があるため、エラー時もリロードして状態を更新
        let result = self.client.stash_apply(index);
        self.report(result, "Stash applied", "Stash apply error", cx);
        self.refresh(cx);
    }

    pub(crate) fn stash_pop(&mut self, index: usize, cx: &mut Context<Self>) {
        let result = self.client.stash_pop(index);
        self.report(result, "Stash popped", "Stash pop error", cx);
        self.refresh(cx);
    }

    pub(crate) fn stash_drop(&mut self, index: usize, cx: &mut Context<Self>) {
        let result = self.client.stash_drop(index);
        self.report(result, "Stash dropped", "Stash drop error", cx);
        self.refresh(cx);
    }

    // ========== リポジトリ一覧のドラッグ&ドロップ ==========

    /// ドロップされたリポジトリを移動する
    pub(crate) fn drop_repo(&mut self, dragged: usize, target: usize, cx: &mut Context<Self>) {
        let zone = self
            .drag_target
            .filter(|(idx, _)| *idx == target)
            .map(|(_, zone)| zone)
            .unwrap_or(DropZone::Center);
        self.drag_target = None;
        if dragged == target {
            cx.notify();
            return;
        }

        let entries = match self.repo_items.get(target) {
            // リスト末尾にドロップ → 末尾に移動
            None => config::move_repo_to_position(dragged, usize::MAX),
            Some(item) => {
                let target_entry = item.original_index as usize;
                if item.is_group {
                    match zone {
                        // グループの上端 → グループの前に挿入
                        DropZone::Before => config::move_repo_to_position(dragged, target_entry),
                        // グループの中心 → グループの末尾に追加
                        DropZone::Center => config::move_repo_to_group(dragged, target_entry),
                        // グループの下端 → グループの先頭に挿入
                        DropZone::After => {
                            config::move_repo_to_group_at_position(dragged, target_entry, 0)
                        }
                    }
                } else if item.indent_level > 0 {
                    // グループ内のリポジトリ → そのグループの該当位置に挿入
                    let mut position = item.index_in_group.max(0) as usize;
                    if zone == DropZone::After {
                        position += 1;
                    }
                    config::move_repo_to_group_at_position(dragged, target_entry, position)
                } else if zone == DropZone::After {
                    config::move_repo_to_position(dragged, target_entry + 1)
                } else {
                    config::move_repo_to_position(dragged, target_entry)
                }
            }
        };
        self.update_repo_entries(entries, cx);
    }
}

pub(crate) fn short_hash(hash: &str) -> &str {
    &hash[..7.min(hash.len())]
}

/// 非同期コンテキストから入力欄の値を設定するヘルパー
trait UpdateInputExt {
    fn update_window_entity_input(&mut self, input: Entity<InputState>, value: String);
}

impl UpdateInputExt for AsyncApp {
    fn update_window_entity_input(&mut self, input: Entity<InputState>, value: String) {
        let Some(window) = self.update(|cx| cx.active_window()) else {
            return;
        };
        window
            .update(self, |_, window, cx| {
                input.update(cx, |input, cx| input.set_value(value, window, cx));
            })
            .ok();
    }
}

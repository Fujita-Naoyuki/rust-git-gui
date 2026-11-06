//! 設定ファイル（最近のリポジトリ・グループ・コミットメッセージ履歴）の読み書き

use serde::{Deserialize, Serialize};
use std::fs;

pub const MAX_RECENT_REPOS: usize = 10;

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

pub fn load_commit_history() -> Vec<String> {
    let path = get_commit_history_path();
    if let Ok(content) = fs::read_to_string(&path) {
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        Vec::new()
    }
}

pub fn save_commit_history(history: &[String]) {
    let path = get_commit_history_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(history) {
        let _ = fs::write(&path, json);
    }
}

pub fn add_recent_repo(path: &str) -> Vec<RepoEntry> {
    let mut entries = load_repo_entries();

    // グループ内に既に存在するかチェック
    let in_group = entries.iter().any(|entry| {
        if let RepoEntry::Group { repos, .. } = entry {
            repos.contains(&path.to_string())
        } else {
            false
        }
    });

    // グループ内にある場合は何もしない（グループ構造を維持）
    if in_group {
        return entries;
    }

    // トップレベルから該当パスを削除
    entries.retain(|entry| match entry {
        RepoEntry::Path(p) => p != path,
        RepoEntry::Group { .. } => true,
    });

    // 先頭に追加
    entries.insert(0, RepoEntry::Path(path.to_string()));

    // 最大数を超えたらトップレベルのパスのみ削除（グループは維持）
    let mut path_count = 0;
    entries.retain(|entry| match entry {
        RepoEntry::Path(_) => {
            path_count += 1;
            path_count <= MAX_RECENT_REPOS
        }
        RepoEntry::Group { .. } => true,
    });

    save_repo_entries(&entries);
    entries
}

/// フラット化したリストのリポジトリを一覧から削除（グループ構造は維持）
pub fn remove_repo_item(item_index: usize) -> Vec<RepoEntry> {
    let mut entries = load_repo_entries();
    let items = flatten_repo_entries(&entries);

    let Some(item) = items.get(item_index) else {
        return entries;
    };
    if item.is_group {
        return entries;
    }

    let entry_idx = item.original_index as usize;
    if item.index_in_group >= 0 {
        if let Some(RepoEntry::Group { repos, .. }) = entries.get_mut(entry_idx) {
            repos.remove(item.index_in_group as usize);
        }
    } else if entry_idx < entries.len() {
        entries.remove(entry_idx);
    }

    save_repo_entries(&entries);
    entries
}

/// 全リポジトリのパスを並び順で取得
pub fn all_repo_paths(entries: &[RepoEntry]) -> Vec<String> {
    entries
        .iter()
        .flat_map(|entry| match entry {
            RepoEntry::Path(p) => vec![p.clone()],
            RepoEntry::Group { repos, .. } => repos.clone(),
        })
        .collect()
}
// ========== リポジトリグループ ==========

/// リポジトリエントリ（単独リポジトリまたはグループ）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RepoEntry {
    /// グループ
    Group {
        #[serde(rename = "type")]
        entry_type: String,
        name: String,
        #[serde(default)]
        collapsed: bool,
        repos: Vec<String>,
    },
    /// 単独リポジトリ
    Path(String),
}

impl RepoEntry {
    fn new_group(name: &str) -> Self {
        RepoEntry::Group {
            entry_type: "group".to_string(),
            name: name.to_string(),
            collapsed: false,
            repos: Vec::new(),
        }
    }
}

/// UI表示用のフラット化されたリストアイテム
#[derive(Debug, Clone)]
pub struct RepoListItem {
    pub is_group: bool,
    pub group_name: String,
    pub group_collapsed: bool,
    pub group_repo_count: i32,
    pub repo_path: String,
    pub repo_name: String, // リポジトリ名（ディレクトリ名）
    pub indent_level: i32,
    pub original_index: i32, // 元のエントリ配列でのインデックス
    pub index_in_group: i32, // グループ内でのインデックス（グループ内のリポジトリの場合）
}

/// 設定ファイルからリポジトリエントリを読み込み（後方互換性あり）
pub fn load_repo_entries() -> Vec<RepoEntry> {
    let path = get_config_path();
    if let Ok(content) = fs::read_to_string(&path) {
        // まず新フォーマット（RepoEntry配列）として解析を試みる
        if let Ok(entries) = serde_json::from_str::<Vec<RepoEntry>>(&content) {
            return entries;
        }
        // 失敗した場合、旧フォーマット（文字列配列）として解析
        if let Ok(paths) = serde_json::from_str::<Vec<String>>(&content) {
            return paths.into_iter().map(RepoEntry::Path).collect();
        }
    }
    Vec::new()
}

/// リポジトリエントリを設定ファイルに保存
pub fn save_repo_entries(entries: &[RepoEntry]) {
    let path = get_config_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(entries) {
        let _ = fs::write(&path, json);
    }
}

/// エントリをUI表示用にフラット化
pub fn flatten_repo_entries(entries: &[RepoEntry]) -> Vec<RepoListItem> {
    let mut items = Vec::new();

    // パスからリポジトリ名（ディレクトリ名）を抽出するヘルパー
    let get_repo_name = |path: &str| -> String {
        std::path::Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(path)
            .to_string()
    };

    for (idx, entry) in entries.iter().enumerate() {
        match entry {
            RepoEntry::Group {
                name,
                collapsed,
                repos,
                ..
            } => {
                // グループヘッダー
                items.push(RepoListItem {
                    is_group: true,
                    group_name: name.clone(),
                    group_collapsed: *collapsed,
                    group_repo_count: repos.len() as i32,
                    repo_path: String::new(),
                    repo_name: String::new(),
                    indent_level: 0,
                    original_index: idx as i32,
                    index_in_group: -1,
                });

                // 折りたたまれていなければグループ内のリポジトリも追加
                if !collapsed {
                    for (repo_idx, repo_path) in repos.iter().enumerate() {
                        items.push(RepoListItem {
                            is_group: false,
                            group_name: name.clone(),
                            group_collapsed: false,
                            group_repo_count: 0,
                            repo_path: repo_path.clone(),
                            repo_name: get_repo_name(repo_path),
                            indent_level: 1,
                            original_index: idx as i32,
                            index_in_group: repo_idx as i32,
                        });
                    }
                }
            }
            RepoEntry::Path(path) => {
                // 単独リポジトリ
                items.push(RepoListItem {
                    is_group: false,
                    group_name: String::new(),
                    group_collapsed: false,
                    group_repo_count: 0,
                    repo_path: path.clone(),
                    repo_name: get_repo_name(path),
                    indent_level: 0,
                    original_index: idx as i32,
                    index_in_group: -1,
                });
            }
        }
    }

    items
}

/// 新しいグループを作成
pub fn create_repo_group(name: &str) -> Vec<RepoEntry> {
    let mut entries = load_repo_entries();
    entries.push(RepoEntry::new_group(name));
    save_repo_entries(&entries);
    entries
}

/// グループの展開/折りたたみを切り替え
pub fn toggle_group_collapsed(index: usize) -> Vec<RepoEntry> {
    let mut entries = load_repo_entries();
    if let Some(RepoEntry::Group { collapsed, .. }) = entries.get_mut(index) {
        *collapsed = !*collapsed;
        save_repo_entries(&entries);
    }
    entries
}

/// グループを削除（中のリポジトリも削除）
pub fn delete_repo_group(index: usize) -> Vec<RepoEntry> {
    let mut entries = load_repo_entries();
    if index < entries.len() {
        if let RepoEntry::Group { .. } = &entries[index] {
            entries.remove(index);
            save_repo_entries(&entries);
        }
    }
    entries
}

/// リポジトリをグループに移動
pub fn move_repo_to_group(item_index: usize, target_group_index: usize) -> Vec<RepoEntry> {
    let mut entries = load_repo_entries();
    let items = flatten_repo_entries(&entries);

    if item_index >= items.len() {
        return entries;
    }

    let item = &items[item_index];
    if item.is_group {
        return entries; // グループは移動対象外
    }

    let repo_path = item.repo_path.clone();
    let source_entry_idx = item.original_index as usize;
    let source_in_group_idx = item.index_in_group;

    // ソースからリポジトリを削除
    if source_in_group_idx >= 0 {
        // グループ内のリポジトリ
        if let Some(RepoEntry::Group { repos, .. }) = entries.get_mut(source_entry_idx) {
            repos.remove(source_in_group_idx as usize);
        }
    } else {
        // トップレベルのリポジトリ
        entries.remove(source_entry_idx);
    }

    // ターゲットグループに追加
    // (削除後にインデックスがずれている可能性があるため、再取得)
    let entries_len = entries.len();
    let adjusted_target = if source_in_group_idx < 0 && source_entry_idx < target_group_index {
        target_group_index.saturating_sub(1)
    } else {
        target_group_index
    };

    if adjusted_target < entries_len {
        if let Some(RepoEntry::Group { repos, .. }) = entries.get_mut(adjusted_target) {
            repos.push(repo_path);
        }
    }

    save_repo_entries(&entries);
    entries
}

/// リポジトリをグループの指定位置に移動
pub fn move_repo_to_group_at_position(
    item_index: usize,
    target_group_index: usize,
    position: usize,
) -> Vec<RepoEntry> {
    let mut entries = load_repo_entries();
    let items = flatten_repo_entries(&entries);

    if item_index >= items.len() {
        return entries;
    }

    let item = &items[item_index];
    if item.is_group {
        return entries; // グループは移動対象外
    }

    let repo_path = item.repo_path.clone();
    let source_entry_idx = item.original_index as usize;
    let source_in_group_idx = item.index_in_group;

    // ソースからリポジトリを削除
    if source_in_group_idx >= 0 {
        // グループ内のリポジトリ
        if let Some(RepoEntry::Group { repos, .. }) = entries.get_mut(source_entry_idx) {
            repos.remove(source_in_group_idx as usize);
        }
    } else {
        // トップレベルのリポジトリ
        entries.remove(source_entry_idx);
    }

    // ターゲットグループの指定位置に挿入
    let entries_len = entries.len();
    let adjusted_target = if source_in_group_idx < 0 && source_entry_idx < target_group_index {
        target_group_index.saturating_sub(1)
    } else {
        target_group_index
    };

    if adjusted_target < entries_len {
        if let Some(RepoEntry::Group { repos, .. }) = entries.get_mut(adjusted_target) {
            let insert_pos = position.min(repos.len());
            repos.insert(insert_pos, repo_path);
        }
    }

    save_repo_entries(&entries);
    entries
}

/// リポジトリをグループから外して特定のエントリ位置に挿入
pub fn move_repo_to_position(item_index: usize, target_entry_index: usize) -> Vec<RepoEntry> {
    let mut entries = load_repo_entries();
    let items = flatten_repo_entries(&entries);

    if item_index >= items.len() {
        return entries;
    }

    let item = &items[item_index];
    if item.is_group {
        return entries; // グループは移動対象外
    }

    let repo_path = item.repo_path.clone();
    let source_entry_idx = item.original_index as usize;
    let source_in_group_idx = item.index_in_group;

    // ソースからリポジトリを削除
    if source_in_group_idx >= 0 {
        // グループ内のリポジトリ
        if let Some(RepoEntry::Group { repos, .. }) = entries.get_mut(source_entry_idx) {
            repos.remove(source_in_group_idx as usize);
        }
    } else {
        // トップレベルのリポジトリ
        entries.remove(source_entry_idx);
    }

    // ターゲット位置に挿入（削除後のインデックス調整）
    let adjusted_target = if source_in_group_idx < 0 && source_entry_idx < target_entry_index {
        target_entry_index.saturating_sub(1)
    } else {
        target_entry_index
    };

    entries.insert(
        adjusted_target.min(entries.len()),
        RepoEntry::Path(repo_path),
    );

    save_repo_entries(&entries);
    entries
}

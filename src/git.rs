//! git2 / gitコマンドによるリポジトリ操作（UI非依存）

use chrono::{DateTime, Local, TimeZone};
use git2::{BranchType, DiffOptions, Oid, Repository, StatusOptions};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::graph::{GraphBuilder, NULL_VERTEX_ID};
use crate::model::*;

// Windowsでコンソールウィンドウを非表示にしてgitコマンドを作成するヘルパー
#[cfg(target_os = "windows")]
pub fn create_git_command() -> std::process::Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    let mut cmd = std::process::Command::new("git");
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

#[cfg(not(target_os = "windows"))]
pub fn create_git_command() -> std::process::Command {
    std::process::Command::new("git")
}

// ========== 別スレッドでのDiff計算 ==========

/// コミットのDiffファイル一覧と最初のファイルのDiff内容を計算する（別スレッド用）
pub fn compute_commit_diff(repo_path: &str, commit_hash: &str) -> (Vec<DiffFileData>, DiffContent) {
    let Ok(repo) = Repository::open(repo_path) else {
        return (vec![], DiffContent::default());
    };

    if commit_hash.is_empty() {
        return (vec![], DiffContent::default());
    }

    let Ok(commit) = repo.find_commit(Oid::from_str(commit_hash).unwrap_or(Oid::zero())) else {
        return (vec![], DiffContent::default());
    };
    let Ok(tree) = commit.tree() else {
        return (vec![], DiffContent::default());
    };

    let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());

    let mut opts = DiffOptions::new();
    let Ok(diff) = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut opts))
    else {
        return (vec![], DiffContent::default());
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
            filename: path,
            status: status.into(),
        });
    }

    // 最初のファイルのDiff内容を取得
    let content = if !files.is_empty() {
        let target_path = files[0].filename.clone();
        let mut opts = DiffOptions::new();
        opts.pathspec(&target_path);
        opts.context_lines(3);

        if let Ok(diff) = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut opts))
        {
            parse_diff(&diff)
        } else {
            DiffContent::default()
        }
    } else {
        DiffContent::default()
    };

    (files, content)
}

/// Diff行数の上限（パフォーマンス対策）
const MAX_DIFF_LINES: usize = 200;
/// カウント上限（これ以上は計算しない）
const MAX_COUNT_LINES: usize = 100000;

/// git2のDiffを表示用の行データに変換する
fn parse_diff(diff: &git2::Diff) -> DiffContent {
    let lines = RefCell::new(vec![]);
    let current_hunk_index = Cell::new(-1i32);
    let truncated = Cell::new(false);
    let total_lines = Cell::new(0usize);

    let _ = diff.print(git2::DiffFormat::Patch, |delta, _hunk, line| {
        // カウント上限チェック
        if total_lines.get() >= MAX_COUNT_LINES {
            return false;
        }
        total_lines.set(total_lines.get() + 1);

        // 表示上限チェック
        if lines.borrow().len() >= MAX_DIFF_LINES {
            truncated.set(true);
            return true; // カウントのために継続
        }

        let kind = match line.origin() {
            '+' => DiffLineKind::Added,
            '-' => DiffLineKind::Removed,
            ' ' => DiffLineKind::Context,
            'H' | 'F' => DiffLineKind::Hunk,
            _ => DiffLineKind::Other,
        };

        if line.origin() == 'H' {
            current_hunk_index.set(current_hunk_index.get() + 1);
        }

        if let Ok(content) = std::str::from_utf8(line.content()) {
            if line.origin() == 'F' {
                if let Some(path) = delta.new_file().path() {
                    lines.borrow_mut().push(DiffLineData {
                        content: format!("--- {}", path.display()),
                        kind: DiffLineKind::File,
                        old_line_num: 0,
                        new_line_num: 0,
                        hunk_index: -1,
                    });
                }
            } else {
                let text = content.trim_end_matches('\n');
                if !text.is_empty() || kind == DiffLineKind::Context {
                    lines.borrow_mut().push(DiffLineData {
                        content: text.to_string(),
                        kind,
                        old_line_num: line.old_lineno().unwrap_or(0),
                        new_line_num: line.new_lineno().unwrap_or(0),
                        hunk_index: current_hunk_index.get(),
                    });
                }
            }
        }
        true
    });

    let mut result = lines.into_inner();

    // 切り捨てメッセージを追加
    if truncated.get() {
        result.push(DiffLineData {
            content: format!(
                "... (truncated: diff exceeds {} lines, view on GitHub for full diff)",
                MAX_DIFF_LINES
            ),
            kind: DiffLineKind::Hunk,
            old_line_num: 0,
            new_line_num: 0,
            hunk_index: -1,
        });
    }

    DiffContent {
        lines: result,
        total_lines: total_lines.get(),
    }
}

// ========== GitClient ==========

pub struct GitClient {
    repo: Option<Repository>,
    repo_path: Option<String>,
}

impl GitClient {
    pub fn new() -> Self {
        Self {
            repo: None,
            repo_path: None,
        }
    }

    pub fn open_repo(&mut self, path: &str) -> Result<(), String> {
        match Repository::open(path) {
            Ok(repo) => {
                self.repo = Some(repo);
                self.repo_path = Some(path.to_string());
                Ok(())
            }
            Err(e) => Err(format!("Failed to open repository: {}", e)),
        }
    }

    pub fn get_current_branch(&self) -> String {
        self.repo.as_ref().map_or("".to_string(), |repo| {
            repo.head()
                .ok()
                .and_then(|h| h.shorthand().map(|s| s.to_string()))
                .unwrap_or_default()
        })
    }

    pub fn get_local_branches(&self) -> Vec<LocalBranchData> {
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
                        name: name.to_string(),
                        is_current: name == current,
                        ahead,
                        behind,
                    });
                }
            }
        }

        branches.sort_by(|a, b| b.is_current.cmp(&a.is_current));
        branches
    }

    /// ローカルブランチとリモートブランチのahead/behindを計算
    pub fn get_ahead_behind(&self, repo: &Repository, branch_name: &str) -> (usize, usize) {
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

    pub fn get_remote_branches(&self) -> Vec<RemoteBranchData> {
        let Some(repo) = &self.repo else {
            return vec![];
        };

        let mut branches = vec![];

        if let Ok(branch_iter) = repo.branches(Some(BranchType::Remote)) {
            for branch in branch_iter.flatten() {
                if let Some(name) = branch.0.name().ok().flatten() {
                    if !name.ends_with("/HEAD") {
                        branches.push(RemoteBranchData {
                            name: name.to_string(),
                        });
                    }
                }
            }
        }

        branches
    }

    /// Git Graphのアルゴリズムでコミットグラフを構築
    pub fn get_commits_with_graph(&self, limit: usize) -> Vec<CommitData> {
        let Some(repo) = &self.repo else {
            return vec![];
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
            return vec![];
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

        // Uncommitted Changesを先頭に追加
        if has_uncommitted {
            commits.push(CommitData {
                hash: "*".into(),
                full_hash: String::new(),
                message: format!("Uncommitted Changes ({})", staged.len() + unstaged.len()),
                author: "*".into(),
                date: Local::now().format("%d %b %H:%M").to_string(),
                branches: vec![],
                color_idx: 0,
                is_merge: false,
                is_uncommitted: true,
                graph: graph_builder.generate_row(0),
            });
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
            let mut commit_branches: Vec<CommitBranchInfo> = branch_names
                .iter()
                .map(|name| CommitBranchInfo {
                    name: name.clone(),
                    is_current: name == &current_branch,
                    is_remote: name.contains('/'),
                })
                .collect();
            commit_branches.sort_by(|a, b| {
                if a.is_current != b.is_current {
                    return b.is_current.cmp(&a.is_current);
                }
                if a.is_remote != b.is_remote {
                    return a.is_remote.cmp(&b.is_remote);
                }
                a.name.cmp(&b.name)
            });

            commits.push(CommitData {
                hash: oid_str[..7].to_string(),
                full_hash: oid_str.clone(),
                message: commit.summary().unwrap_or("").to_string(),
                author: commit.author().name().unwrap_or("").to_string(),
                date: datetime.format("%d %b %H:%M").to_string(),
                branches: commit_branches,
                color_idx: graph_builder.get_vertex_colour(row),
                is_merge: graph_builder.is_vertex_merge(row),
                is_uncommitted: false,
                graph: graph_builder.generate_row(row),
            });
        }

        commits
    }

    pub fn get_status(&self) -> (Vec<FileData>, Vec<FileData>) {
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
                        filename: path.clone(),
                        status: "A".into(),
                    });
                } else if status.is_index_modified() {
                    staged.push(FileData {
                        filename: path.clone(),
                        status: "M".into(),
                    });
                } else if status.is_index_deleted() {
                    staged.push(FileData {
                        filename: path.clone(),
                        status: "D".into(),
                    });
                } else if status.is_index_renamed() {
                    staged.push(FileData {
                        filename: path.clone(),
                        status: "R".into(),
                    });
                }

                if status.is_wt_new() {
                    unstaged.push(FileData {
                        filename: path.clone(),
                        status: "?".into(),
                    });
                } else if status.is_wt_modified() {
                    unstaged.push(FileData {
                        filename: path.clone(),
                        status: "M".into(),
                    });
                } else if status.is_wt_deleted() {
                    unstaged.push(FileData {
                        filename: path,
                        status: "D".into(),
                    });
                }
            }
        }
        (staged, unstaged)
    }

    pub fn stage_file(&self, filename: &str) -> Result<(), String> {
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

    pub fn stage_all(&self) -> Result<(), String> {
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

    pub fn unstage_file(&self, filename: &str) -> Result<(), String> {
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

    pub fn unstage_all(&self) -> Result<(), String> {
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

    pub fn commit(&self, message: &str) -> Result<(), String> {
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

    pub fn checkout_branch(&self, name: &str) -> Result<(), String> {
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

    pub fn checkout_remote_branch(&self, remote_name: &str) -> Result<(), String> {
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

    pub fn create_branch(&self, name: &str) -> Result<(), String> {
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

    pub fn create_branch_at_commit(&self, name: &str, commit_hash: &str) -> Result<(), String> {
        let Some(repo) = &self.repo else {
            return Err("No repository".into());
        };

        let oid = Oid::from_str(commit_hash).map_err(|e| e.to_string())?;
        let commit = repo.find_commit(oid).map_err(|e| e.to_string())?;

        repo.branch(name, &commit, false)
            .map_err(|e| e.to_string())?;
        self.checkout_branch(name)?;
        Ok(())
    }

    pub fn delete_branch(&self, name: &str) -> Result<(), String> {
        let Some(repo) = &self.repo else {
            return Err("No repository".into());
        };

        let mut branch = repo
            .find_branch(name, BranchType::Local)
            .map_err(|e| e.to_string())?;
        branch.delete().map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn merge_branch(&self, name: &str) -> Result<(), String> {
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

    pub fn get_stashes(&mut self) -> Vec<StashData> {
        let Some(repo) = &mut self.repo else {
            return vec![];
        };
        let mut stashes = vec![];
        let _ = repo.stash_foreach(|index, name, _oid| {
            stashes.push(StashData {
                index,
                message: name.to_string(),
            });
            true
        });
        stashes
    }

    pub fn stash_save(&mut self, message: &str, include_untracked: bool) -> Result<(), String> {
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

    pub fn stash_apply(&mut self, index: usize) -> Result<(), String> {
        let Some(repo) = &mut self.repo else {
            return Err("No repository".into());
        };
        let mut options = git2::StashApplyOptions::default();
        repo.stash_apply(index, Some(&mut options))
            .map_err(|e| e.to_string())
    }

    pub fn stash_pop(&mut self, index: usize) -> Result<(), String> {
        let Some(repo) = &mut self.repo else {
            return Err("No repository".into());
        };
        let mut options = git2::StashApplyOptions::default();
        repo.stash_pop(index, Some(&mut options))
            .map_err(|e| e.to_string())
    }

    pub fn stash_drop(&mut self, index: usize) -> Result<(), String> {
        let Some(repo) = &mut self.repo else {
            return Err("No repository".into());
        };
        repo.stash_drop(index).map_err(|e| e.to_string())
    }

    pub fn get_commit_file_diff(&self, oid: &str, file_index: usize) -> DiffContent {
        let Some(repo) = &self.repo else {
            return DiffContent::default();
        };

        if oid.is_empty() {
            return DiffContent::default();
        }

        let Ok(commit) = repo.find_commit(Oid::from_str(oid).unwrap_or(Oid::zero())) else {
            return DiffContent::default();
        };
        let Ok(tree) = commit.tree() else {
            return DiffContent::default();
        };

        let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());

        let mut opts = DiffOptions::new();
        let Ok(diff) = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut opts))
        else {
            return DiffContent::default();
        };

        let deltas: Vec<_> = diff.deltas().collect();
        if file_index >= deltas.len() {
            return DiffContent::default();
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
            return DiffContent::default();
        };

        parse_diff(&diff)
    }

    pub fn get_file_diff(&self, filename: &str, staged: bool) -> DiffContent {
        let Some(repo) = &self.repo else {
            return DiffContent::default();
        };

        let mut opts = DiffOptions::new();
        opts.pathspec(filename);
        opts.context_lines(3);

        let diff = if staged {
            let Ok(head_tree) = repo.head().and_then(|h| h.peel_to_tree()) else {
                return DiffContent::default();
            };
            repo.diff_tree_to_index(Some(&head_tree), None, Some(&mut opts))
        } else {
            // Include untracked files in diff
            opts.include_untracked(true);

            repo.diff_index_to_workdir(None, Some(&mut opts))
        };

        match diff {
            Ok(d) => {
                let content = parse_diff(&d);
                // If no diff lines but it's an unstaged file, it might be untracked (new file)
                // Read the file content directly and show as all additions
                if content.lines.is_empty() && !staged {
                    return self.get_new_file_diff(repo, filename);
                }
                content
            }
            Err(_) => {
                // If diff failed and it's unstaged, try reading as new file
                if !staged {
                    return self.get_new_file_diff(repo, filename);
                }
                DiffContent::default()
            }
        }
    }

    /// Get diff for a new (untracked) file by reading its contents
    fn get_new_file_diff(&self, repo: &Repository, filename: &str) -> DiffContent {
        let Some(workdir) = repo.workdir() else {
            return DiffContent::default();
        };

        let file_path = workdir.join(filename);
        let content = match fs::read_to_string(&file_path) {
            Ok(c) => c,
            Err(_) => {
                // Try reading as binary and show a placeholder message
                if fs::read(&file_path).is_err() {
                    return DiffContent::default();
                }
                return DiffContent {
                    lines: vec![DiffLineData {
                        content: "(Binary file)".into(),
                        kind: DiffLineKind::Context,
                        old_line_num: 0,
                        new_line_num: 0,
                        hunk_index: 0,
                    }],
                    total_lines: 1,
                };
            }
        };

        let header = |content: String, kind: DiffLineKind, hunk_index: i32| DiffLineData {
            content,
            kind,
            old_line_num: 0,
            new_line_num: 0,
            hunk_index,
        };

        let line_count = content.lines().count();
        let mut lines = vec![
            header("--- /dev/null".into(), DiffLineKind::File, -1),
            header(format!("+++ {}", filename), DiffLineKind::File, -1),
            header(
                format!("@@ -0,0 +1,{} @@", line_count),
                DiffLineKind::Hunk,
                0,
            ),
        ];

        // Add all lines as additions
        for (i, line) in content.lines().take(MAX_DIFF_LINES).enumerate() {
            lines.push(DiffLineData {
                content: line.to_string(),
                kind: DiffLineKind::Added,
                old_line_num: 0,
                new_line_num: (i + 1) as u32,
                hunk_index: 0,
            });
        }

        DiffContent {
            total_lines: line_count,
            lines,
        }
    }

    /// 特定のHunkをステージングする
    pub fn stage_hunk(&self, filename: &str, hunk_index: usize) -> Result<(), String> {
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

    pub fn discard_file(&self, filename: &str) -> Result<(), String> {
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
    pub fn push(&self) -> Result<(), String> {
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
    pub fn pull(&self) -> Result<(), String> {
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
    pub fn get_github_url(&self) -> Option<String> {
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
    pub fn get_pull_request_url(&self, branch_name: &str) -> Option<String> {
        let github_url = self.get_github_url()?;
        // GitHub PR作成URL: https://github.com/user/repo/compare/main...branch?expand=1
        Some(format!(
            "{}/compare/main...{}?expand=1",
            github_url, branch_name
        ))
    }

    /// コミットのGitHub URLを生成
    pub fn get_commit_github_url(&self, commit_hash: &str) -> Option<String> {
        let github_url = self.get_github_url()?;
        Some(format!("{}/commit/{}", github_url, commit_hash))
    }

    /// 指定したコミットにリセット
    pub fn reset_to_commit(&self, commit_hash: &str, mode: &str) -> Result<(), String> {
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
    pub fn revert_commit(&self, commit_hash: &str) -> Result<(), String> {
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
}

/// 画面表示に必要なリポジトリの状態一式（バックグラウンドで構築する）
#[derive(Debug, Clone, Default)]
pub struct RepoSnapshot {
    pub current_branch: String,
    pub local_branches: Vec<LocalBranchData>,
    pub remote_branches: Vec<RemoteBranchData>,
    pub stashes: Vec<StashData>,
    pub commits: Vec<CommitData>,
    pub staged: Vec<FileData>,
    pub unstaged: Vec<FileData>,
}

/// コミットグラフに表示する最大コミット数
pub const MAX_GRAPH_COMMITS: usize = 300;

/// リポジトリを開き直して状態を読み込む（別スレッド用）
pub fn load_snapshot(repo_path: &str) -> Result<RepoSnapshot, String> {
    let mut client = GitClient::new();
    client.open_repo(repo_path)?;
    let (staged, unstaged) = client.get_status();
    Ok(RepoSnapshot {
        current_branch: client.get_current_branch(),
        local_branches: client.get_local_branches(),
        remote_branches: client.get_remote_branches(),
        stashes: client.get_stashes(),
        commits: client.get_commits_with_graph(MAX_GRAPH_COMMITS),
        staged,
        unstaged,
    })
}

/// gitコマンドを実行し、失敗時は標準エラー出力をエラーとして返す
fn run_git(args: &[&str], cwd: Option<&str>, label: &str) -> Result<(), String> {
    let mut cmd = create_git_command();
    cmd.args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let output = cmd
        .output()
        .map_err(|e| format!("{} error: {}", label, e))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("{} failed: {}", label, stderr.trim()))
    }
}

/// 全リモートからFetch
pub fn fetch_all(repo_path: &str) -> Result<(), String> {
    run_git(&["fetch", "--all"], Some(repo_path), "Fetch")
}

/// リポジトリをクローンし、クローン先のパスを返す
pub fn clone_repo(url: &str, dest: &str) -> Result<String, String> {
    let mut dest = dest.to_string();
    // スマートパス補完: 指定されたパスが存在し、かつ空でない場合はリポジトリ名を追加
    let path = Path::new(&dest);
    if path.exists()
        && path
            .read_dir()
            .map(|mut i| i.next().is_some())
            .unwrap_or(false)
    {
        // URLからリポジトリ名を抽出 (e.g. https://github.com/user/repo.git -> repo)
        let repo_name = url
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .map(|s| s.trim_end_matches(".git"))
            .filter(|s| !s.is_empty())
            .unwrap_or("repository");
        dest = path.join(repo_name).to_string_lossy().to_string();
    }

    run_git(&["clone", url, &dest], None, "Clone")?;
    Ok(dest)
}

/// 現在のブランチをプッシュ（別スレッド用）
pub fn push_repo(repo_path: &str) -> Result<(), String> {
    let mut client = GitClient::new();
    client.open_repo(repo_path)?;
    client.push()
}

/// プル（別スレッド用）
pub fn pull_repo(repo_path: &str) -> Result<(), String> {
    let mut client = GitClient::new();
    client.open_repo(repo_path)?;
    client.pull()
}

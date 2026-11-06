//! UIに依存しないデータ構造

use crate::graph::GraphRow;

/// Graph用の色パレット
pub const GRAPH_COLORS: [u32; 16] = [
    0x3584e4, // Blue
    0x2ec27e, // Green
    0xf5c211, // Yellow
    0xe01b24, // Red
    0x9141ac, // Purple
    0xff7800, // Orange
    0x00b8d4, // Cyan
    0xe91e63, // Pink
    0x4fc3f7, // Light Blue
    0x81c784, // Light Green
    0xffb74d, // Light Orange
    0xf06292, // Light Pink
    0xba68c8, // Light Purple
    0x4db6ac, // Teal
    0xaed581, // Lime
    0x90a4ae, // Blue Grey
];

pub fn graph_color(idx: usize) -> u32 {
    GRAPH_COLORS[idx % GRAPH_COLORS.len()]
}

#[derive(Debug, Clone)]
pub struct StashData {
    pub index: usize,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct CommitBranchInfo {
    pub name: String,
    pub is_current: bool,
    pub is_remote: bool,
}

#[derive(Debug, Clone)]
pub struct CommitData {
    pub hash: String,
    pub full_hash: String,
    pub message: String,
    pub author: String,
    pub date: String,
    pub branches: Vec<CommitBranchInfo>,
    pub color_idx: usize,
    pub is_merge: bool,
    pub is_uncommitted: bool,
    /// この行に描画するグラフ要素
    pub graph: GraphRow,
}

#[derive(Debug, Clone)]
pub struct FileData {
    pub filename: String,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct LocalBranchData {
    pub name: String,
    pub is_current: bool,
    pub ahead: usize,
    pub behind: usize,
}

#[derive(Debug, Clone)]
pub struct RemoteBranchData {
    pub name: String,
}

/// Diff行の種類
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    /// ファイルヘッダー（--- path）
    File,
    /// Hunkヘッダー（@@ ... @@）
    Hunk,
    Added,
    Removed,
    Context,
    Other,
}

#[derive(Debug, Clone)]
pub struct DiffLineData {
    pub content: String,
    pub kind: DiffLineKind,
    /// 0は行番号なし
    pub old_line_num: u32,
    pub new_line_num: u32,
    /// 所属するHunkのインデックス（-1はHunk外）
    pub hunk_index: i32,
}

#[derive(Debug, Clone)]
pub struct DiffFileData {
    pub filename: String,
    pub status: String,
}

/// Diff表示の内容（表示行と総行数）
#[derive(Debug, Clone, Default)]
pub struct DiffContent {
    pub lines: Vec<DiffLineData>,
    pub total_lines: usize,
}

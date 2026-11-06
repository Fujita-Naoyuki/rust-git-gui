//! Git Graph（VSCode Git Graph互換）のレーン計算

/// グラフ描画の定数
pub const COL_SPACING: f32 = 16.0;
pub const ROW_HEIGHT: f32 = 28.0;
pub const NODE_RADIUS: f32 = 4.0;
const NODE_CENTER_Y: f32 = ROW_HEIGHT / 2.0;
const CURVE_OFFSET: f32 = ROW_HEIGHT * 0.8;
const X_OFFSET: f32 = 7.0;

/// 1行分のグラフ描画要素（行ローカル座標）
#[derive(Debug, Clone, Default)]
pub struct GraphRow {
    pub segments: Vec<GraphSegment>,
    /// ノード中心のX座標
    pub node_x: f32,
}

#[derive(Debug, Clone, Copy)]
pub enum GraphShape {
    Line {
        from: (f32, f32),
        to: (f32, f32),
    },
    Curve {
        from: (f32, f32),
        ctrl_a: (f32, f32),
        ctrl_b: (f32, f32),
        to: (f32, f32),
    },
}

#[derive(Debug, Clone, Copy)]
pub struct GraphSegment {
    pub color_idx: usize,
    pub shape: GraphShape,
}

// ========== Git Graphのデータ構造 ==========

pub const NULL_VERTEX_ID: i32 = -1;

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
pub struct GraphBuilder {
    vertices: Vec<Vertex>,
    branches: Vec<Branch>,
    available_colours: Vec<usize>,
}

impl GraphBuilder {
    pub fn new() -> Self {
        Self {
            vertices: Vec::new(),
            branches: Vec::new(),
            available_colours: Vec::new(),
        }
    }

    /// コミットデータからグラフを構築
    pub fn load_commits(
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

    /// 指定行のグラフ描画要素を生成
    pub fn generate_row(&self, row: usize) -> GraphRow {
        let mut segments = Vec::new();

        // このコミットを通過する全ブランチの線を描画
        for branch in self.branches.iter() {
            let color_idx = branch.get_colour();

            for line in &branch.lines {
                // この行に関係する線のみ処理
                if line.p1.y as usize == row
                    || line.p2.y as usize == row
                    || (line.p1.y < row as i32 && line.p2.y > row as i32)
                {
                    let x1 = line.p1.x as f32 * COL_SPACING + X_OFFSET;
                    let y1 = line.p1.y as f32 * ROW_HEIGHT + NODE_CENTER_Y;
                    let x2 = line.p2.x as f32 * COL_SPACING + X_OFFSET;
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
                            segments.push(GraphSegment {
                                color_idx,
                                shape: GraphShape::Line {
                                    from: (x1, draw_y1 - row_top),
                                    to: (x1, draw_y2 - row_top),
                                },
                            });
                        }
                    } else if line.p1.y as usize == row || line.p2.y as usize == row {
                        // 曲線（この行が始点または終点の場合のみ）
                        segments.push(GraphSegment {
                            color_idx,
                            shape: Self::curve_segment(line, row),
                        });
                    }
                }
            }
        }

        let node_x = if row < self.vertices.len() {
            self.vertices[row].x as f32 * COL_SPACING + X_OFFSET
        } else {
            X_OFFSET
        };

        GraphRow { segments, node_x }
    }

    fn curve_segment(line: &Line, row: usize) -> GraphShape {
        let x1 = line.p1.x as f32 * COL_SPACING + X_OFFSET;
        let x2 = line.p2.x as f32 * COL_SPACING + X_OFFSET;

        if line.p1.y as usize == row {
            // この行が始点
            let local_y1 = NODE_CENTER_Y;
            let local_y2 = ROW_HEIGHT;

            if line.locked_first {
                // 上に固定: 曲線は下に向かう
                let ctrl_y = local_y1 + CURVE_OFFSET.min(ROW_HEIGHT - NODE_CENTER_Y);
                GraphShape::Curve {
                    from: (x1, local_y1),
                    ctrl_a: (x1, ctrl_y),
                    ctrl_b: (x2, local_y2),
                    to: (x2, local_y2),
                }
            } else {
                // 下に固定: 直線で下へ、次の行で曲がる
                GraphShape::Line {
                    from: (x1, local_y1),
                    to: (x1, local_y2),
                }
            }
        } else {
            // この行が終点
            let local_y1 = 0.0;
            let local_y2 = NODE_CENTER_Y;

            if line.locked_first {
                // 上に固定: 直線で上から来る
                GraphShape::Line {
                    from: (x2, local_y1),
                    to: (x2, local_y2),
                }
            } else {
                // 下に固定: 曲線で終点に向かう
                let ctrl_y = local_y2 - CURVE_OFFSET.min(NODE_CENTER_Y);
                GraphShape::Curve {
                    from: (x1, local_y1),
                    ctrl_a: (x1, local_y1),
                    ctrl_b: (x2, ctrl_y),
                    to: (x2, local_y2),
                }
            }
        }
    }

    pub fn get_vertex_colour(&self, row: usize) -> usize {
        if row < self.vertices.len() {
            self.vertices[row].get_colour(&self.branches)
        } else {
            0
        }
    }

    pub fn is_vertex_merge(&self, row: usize) -> bool {
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

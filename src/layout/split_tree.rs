//! Pane layout of one tab: a tree of row/column splits with fractional sizes.
//! Pure geometry, no egui, so every operation is unit-tested.

use serde::{Deserialize, Serialize};

pub type PaneId = u64;

/// `Row` lays children left to right, `Column` top to bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dir {
    Row,
    Column,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Node {
    Leaf(PaneId),
    /// Fractions of the children sum to 1.
    Split { dir: Dir, children: Vec<(f32, Node)> },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavDir {
    Left,
    Right,
    Up,
    Down,
}

/// Where a removed pane was: next to `neighbor` along `dir`, after it or before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub neighbor: PaneId,
    pub dir: Dir,
    pub after: bool,
}

/// The gap between child `index` and `index + 1` of the split at `path`.
#[derive(Clone, Debug, PartialEq)]
pub struct Divider {
    pub path: Vec<usize>,
    pub index: usize,
    pub dir: Dir,
    pub rect: Rect,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SplitTree {
    root: Node,
}

impl SplitTree {
    pub fn new(pane: PaneId) -> SplitTree {
        SplitTree { root: Node::Leaf(pane) }
    }

    /// Builds a tree from saved state, fixing fractions that do not sum to 1
    /// and splits with fewer than two children.
    pub fn from_root(mut root: Node) -> SplitTree {
        normalize(&mut root);
        collapse(&mut root);
        SplitTree { root }
    }

    pub fn root(&self) -> &Node {
        &self.root
    }

    /// Panes in reading order (depth-first, left to right, top to bottom).
    pub fn panes(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        collect(&self.root, &mut out);
        out
    }

    pub fn contains(&self, id: PaneId) -> bool {
        fn contains(node: &Node, id: PaneId) -> bool {
            match node {
                Node::Leaf(pane) => *pane == id,
                Node::Split { children, .. } => children.iter().any(|(_, child)| contains(child, id)),
            }
        }
        contains(&self.root, id)
    }

    pub fn pane_count(&self) -> usize {
        fn count(node: &Node) -> usize {
            match node {
                Node::Leaf(_) => 1,
                Node::Split { children, .. } => children.iter().map(|(_, child)| count(child)).sum(),
            }
        }
        count(&self.root)
    }

    /// Puts `new` next to `target` along `dir`, after it (right/below) or before.
    pub fn insert(&mut self, target: PaneId, new: PaneId, dir: Dir, after: bool) -> bool {
        insert_in(&mut self.root, target, new, dir, after)
    }

    pub fn split_right(&mut self, target: PaneId, new: PaneId) -> bool {
        self.insert(target, new, Dir::Row, true)
    }

    pub fn split_down(&mut self, target: PaneId, new: PaneId) -> bool {
        self.insert(target, new, Dir::Column, true)
    }

    /// Removes `id` and returns where it was. The last pane is never removed.
    pub fn remove(&mut self, id: PaneId) -> Option<Anchor> {
        if matches!(self.root, Node::Leaf(_)) {
            return None;
        }
        let anchor = remove_in(&mut self.root, id)?;
        collapse(&mut self.root);
        Some(anchor)
    }

    /// Moves `id` beside `target`, or to the outer edge of the remaining tree
    /// when `target` is `None`. Right/below is `after`, left/above is before.
    /// The moved pane shares its target's space equally, or takes half of the
    /// whole tab at an outer edge. Invalid and self drops leave the tree unchanged.
    pub fn relocate(&mut self, id: PaneId, target: Option<PaneId>, dir: Dir, after: bool) -> bool {
        if target == Some(id)
            || !self.contains(id)
            || matches!(self.root, Node::Leaf(_))
            || target.is_some_and(|target| !self.contains(target))
        {
            return false;
        }
        if self.remove(id).is_none() {
            return false;
        }
        if let Some(target) = target {
            self.insert(target, id, dir, after)
        } else {
            let remaining = std::mem::replace(&mut self.root, Node::Leaf(id));
            let moved = Node::Leaf(id);
            let children =
                if after { vec![(0.5, remaining), (0.5, moved)] } else { vec![(0.5, moved), (0.5, remaining)] };
            self.root = Node::Split { dir, children };
            true
        }
    }

    /// Puts a removed pane back where it was, or right of `fallback` when its
    /// old neighbour is gone.
    pub fn restore(&mut self, id: PaneId, anchor: Anchor, fallback: PaneId) -> bool {
        if self.contains(anchor.neighbor) {
            self.insert(anchor.neighbor, id, anchor.dir, anchor.after)
        } else {
            self.split_right(fallback, id)
        }
    }

    /// Pane rectangles inside `area`, with `gap` between siblings.
    pub fn layout(&self, area: Rect, gap: f32) -> Vec<(PaneId, Rect)> {
        let mut out = Vec::new();
        walk(&self.root, area, gap, &mut Vec::new(), &mut |node, rect, _path, _| {
            if let Node::Leaf(id) = node {
                out.push((*id, rect));
            }
        });
        out
    }

    pub fn dividers(&self, area: Rect, gap: f32) -> Vec<Divider> {
        let mut out = Vec::new();
        walk(&self.root, area, gap, &mut Vec::new(), &mut |node, rect, path, child_rects| {
            if let Node::Split { dir, .. } = node {
                for (index, pair) in child_rects.windows(2).enumerate() {
                    let (a, b) = (pair[0], pair[1]);
                    let r = match dir {
                        Dir::Row => Rect::new(a.right(), rect.y, b.x - a.right(), rect.h),
                        Dir::Column => Rect::new(rect.x, a.bottom(), rect.w, b.y - a.bottom()),
                    };
                    out.push(Divider { path: path.to_vec(), index, dir: *dir, rect: r });
                }
            }
        });
        out
    }

    /// Moves a divider by `delta` pixels, keeping both neighbours at least
    /// `min_size` pixels along the split axis.
    pub fn drag_divider(&mut self, area: Rect, gap: f32, divider: &Divider, delta: f32, min_size: f32) {
        let Some(rect) = self.node_rect(area, gap, &divider.path) else { return };
        let Some(Node::Split { dir, children }) = node_at_mut(&mut self.root, &divider.path) else { return };
        if divider.index + 1 >= children.len() {
            return;
        }
        let total = match dir {
            Dir::Row => rect.w,
            Dir::Column => rect.h,
        };
        let avail = total - gap * (children.len() as f32 - 1.0);
        if avail <= 0.0 {
            return;
        }
        let a = children[divider.index].0 * avail;
        let b = children[divider.index + 1].0 * avail;
        let lo = -(a - min_size).max(0.0);
        let hi = (b - min_size).max(0.0);
        let d = delta.clamp(lo, hi);
        children[divider.index].0 = (a + d) / avail;
        children[divider.index + 1].0 = (b - d) / avail;
    }

    /// The nearest pane in `dir` that overlaps `from` on the other axis; ties
    /// go to the pane whose centre is closest, then to reading order.
    pub fn navigate(&self, from: PaneId, dir: NavDir, area: Rect, gap: f32) -> Option<PaneId> {
        let rects = self.layout(area, gap);
        let (_, f) = *rects.iter().find(|(id, _)| *id == from)?;
        const EPS: f32 = 1.0;
        let overlap = |a0: f32, a1: f32, b0: f32, b1: f32| a1.min(b1) - a0.max(b0) > 0.0;
        let mut best: Option<(f32, f32, PaneId)> = None;
        for (id, r) in &rects {
            if *id == from {
                continue;
            }
            let (distance, offcenter) = match dir {
                NavDir::Left if r.right() <= f.x + EPS && overlap(r.y, r.bottom(), f.y, f.bottom()) => {
                    (f.x - r.right(), ((r.y + r.h / 2.0) - (f.y + f.h / 2.0)).abs())
                }
                NavDir::Right if r.x >= f.right() - EPS && overlap(r.y, r.bottom(), f.y, f.bottom()) => {
                    (r.x - f.right(), ((r.y + r.h / 2.0) - (f.y + f.h / 2.0)).abs())
                }
                NavDir::Up if r.bottom() <= f.y + EPS && overlap(r.x, r.right(), f.x, f.right()) => {
                    (f.y - r.bottom(), ((r.x + r.w / 2.0) - (f.x + f.w / 2.0)).abs())
                }
                NavDir::Down if r.y >= f.bottom() - EPS && overlap(r.x, r.right(), f.x, f.right()) => {
                    (r.y - f.bottom(), ((r.x + r.w / 2.0) - (f.x + f.w / 2.0)).abs())
                }
                _ => continue,
            };
            let better = match best {
                None => true,
                Some((bd, bo, _)) => {
                    distance < bd - 0.5 || ((distance - bd).abs() <= 0.5 && offcenter < bo - 0.5)
                }
            };
            if better {
                best = Some((distance, offcenter, *id));
            }
        }
        best.map(|(_, _, id)| id)
    }

    pub fn next(&self, from: PaneId) -> Option<PaneId> {
        let panes = self.panes();
        let i = panes.iter().position(|p| *p == from)?;
        Some(panes[(i + 1) % panes.len()])
    }

    pub fn previous(&self, from: PaneId) -> Option<PaneId> {
        let panes = self.panes();
        let i = panes.iter().position(|p| *p == from)?;
        Some(panes[(i + panes.len() - 1) % panes.len()])
    }

    fn node_rect(&self, area: Rect, gap: f32, target: &[usize]) -> Option<Rect> {
        let mut found = None;
        walk(&self.root, area, gap, &mut Vec::new(), &mut |_, rect, path, _| {
            if path == target {
                found = Some(rect);
            }
        });
        found
    }
}

fn collect(node: &Node, out: &mut Vec<PaneId>) {
    match node {
        Node::Leaf(id) => out.push(*id),
        Node::Split { children, .. } => children.iter().for_each(|(_, c)| collect(c, out)),
    }
}

fn first_leaf(node: &Node) -> PaneId {
    match node {
        Node::Leaf(id) => *id,
        Node::Split { children, .. } => first_leaf(&children[0].1),
    }
}

fn last_leaf(node: &Node) -> PaneId {
    match node {
        Node::Leaf(id) => *id,
        Node::Split { children, .. } => last_leaf(&children[children.len() - 1].1),
    }
}

fn insert_in(node: &mut Node, target: PaneId, new: PaneId, dir: Dir, after: bool) -> bool {
    match node {
        Node::Leaf(id) if *id == target => {
            let (old, fresh) = (Node::Leaf(target), Node::Leaf(new));
            let children = if after { vec![(0.5, old), (0.5, fresh)] } else { vec![(0.5, fresh), (0.5, old)] };
            *node = Node::Split { dir, children };
            true
        }
        Node::Leaf(_) => false,
        Node::Split { dir: d, children } => {
            if *d == dir {
                if let Some(i) = children.iter().position(|(_, c)| matches!(c, Node::Leaf(id) if *id == target)) {
                    let half = children[i].0 / 2.0;
                    children[i].0 = half;
                    children.insert(if after { i + 1 } else { i }, (half, Node::Leaf(new)));
                    return true;
                }
            }
            children.iter_mut().any(|(_, c)| insert_in(c, target, new, dir, after))
        }
    }
}

fn remove_in(node: &mut Node, id: PaneId) -> Option<Anchor> {
    let Node::Split { dir, children } = node else { return None };
    if let Some(i) = children.iter().position(|(_, c)| matches!(c, Node::Leaf(x) if *x == id)) {
        children.remove(i);
        let anchor = if i > 0 {
            Anchor { neighbor: last_leaf(&children[i - 1].1), dir: *dir, after: true }
        } else {
            Anchor { neighbor: first_leaf(&children[0].1), dir: *dir, after: false }
        };
        let rest: f32 = children.iter().map(|(f, _)| *f).sum();
        if rest > 0.0 {
            children.iter_mut().for_each(|(f, _)| *f /= rest);
        }
        return Some(anchor);
    }
    children.iter_mut().find_map(|(_, c)| remove_in(c, id))
}

fn collapse(node: &mut Node) {
    if let Node::Split { children, .. } = node {
        children.iter_mut().for_each(|(_, c)| collapse(c));
        // A split with no panes (a hand-edited session.json) has no first or
        // last leaf to anchor to: drop it rather than panic on it later.
        children.retain(|(_, c)| !matches!(c, Node::Split { children, .. } if children.is_empty()));
        if children.len() == 1 {
            let only = children.pop().expect("one child").1;
            *node = only;
        }
    }
}

fn normalize(node: &mut Node) {
    if let Node::Split { children, .. } = node {
        let sum: f32 = children.iter().map(|(f, _)| f.max(0.0)).sum();
        let n = children.len() as f32;
        for (f, c) in children.iter_mut() {
            *f = if sum > 0.0 { f.max(0.0) / sum } else { 1.0 / n };
            normalize(c);
        }
    }
}

fn node_at_mut<'a>(node: &'a mut Node, path: &[usize]) -> Option<&'a mut Node> {
    match path.split_first() {
        None => Some(node),
        Some((i, rest)) => match node {
            Node::Split { children, .. } => node_at_mut(&mut children.get_mut(*i)?.1, rest),
            Node::Leaf(_) => None,
        },
    }
}

/// Called with a node, its rectangle, its path and (for splits) its children's rectangles.
type Visitor<'a> = dyn FnMut(&Node, Rect, &[usize], &[Rect]) + 'a;

/// Visits every node with its rectangle and path; for splits also passes the
/// rectangles of its children (the last child takes the rounding remainder).
fn walk(node: &Node, rect: Rect, gap: f32, path: &mut Vec<usize>, visit: &mut Visitor<'_>) {
    match node {
        Node::Leaf(_) => visit(node, rect, path, &[]),
        Node::Split { dir, children } => {
            let n = children.len();
            let (start, total) = match dir {
                Dir::Row => (rect.x, rect.w),
                Dir::Column => (rect.y, rect.h),
            };
            let avail = (total - gap * (n as f32 - 1.0)).max(0.0);
            let mut pos = start;
            let mut rects = Vec::with_capacity(n);
            for (i, (f, _)) in children.iter().enumerate() {
                let size = if i + 1 == n { (start + total - pos).max(0.0) } else { avail * f };
                rects.push(match dir {
                    Dir::Row => Rect::new(pos, rect.y, size, rect.h),
                    Dir::Column => Rect::new(rect.x, pos, rect.w, size),
                });
                pos += size + gap;
            }
            visit(node, rect, path, &rects);
            for (i, (_, child)) in children.iter().enumerate() {
                path.push(i);
                walk(child, rects[i], gap, path, visit);
                path.pop();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect { x: 0.0, y: 0.0, w: 306.0, h: 206.0 };
    const GAP: f32 = 6.0;

    fn row(children: Vec<(f32, Node)>) -> Node {
        Node::Split { dir: Dir::Row, children }
    }
    fn col(children: Vec<(f32, Node)>) -> Node {
        Node::Split { dir: Dir::Column, children }
    }
    fn leaf(id: PaneId) -> Node {
        Node::Leaf(id)
    }

    fn relocation_tree() -> SplitTree {
        SplitTree::from_root(row(vec![
            (0.4, col(vec![(0.25, leaf(1)), (0.75, leaf(2))])),
            (0.6, col(vec![(0.5, leaf(3)), (0.5, leaf(4))])),
        ]))
    }

    fn assert_relocation_invariants(tree: &SplitTree, expected_ids: &[PaneId]) {
        fn check(node: &Node) {
            if let Node::Split { children, .. } = node {
                assert!(children.len() >= 2, "single-child splits must collapse");
                let sum: f32 = children.iter().map(|(weight, _)| *weight).sum();
                assert!((sum - 1.0).abs() < 1e-6, "fractions sum to {sum}");
                for (weight, child) in children {
                    assert!(weight.is_finite() && *weight > 0.0, "invalid fraction {weight}");
                    check(child);
                }
            }
        }
        let mut ids = tree.panes();
        ids.sort_unstable();
        assert_eq!(ids, expected_ids, "each pane must occur exactly once");
        check(tree.root());
    }

    #[test]
    fn relocate_nested_pane_in_all_four_directions() {
        for dir in [Dir::Row, Dir::Column] {
            for after in [false, true] {
                let mut tree = relocation_tree();
                assert!(tree.relocate(1, Some(3), dir, after));
                let target_children = match dir {
                    Dir::Row => {
                        let pair = if after {
                            vec![(0.5, leaf(3)), (0.5, leaf(1))]
                        } else {
                            vec![(0.5, leaf(1)), (0.5, leaf(3))]
                        };
                        vec![(0.5, row(pair)), (0.5, leaf(4))]
                    }
                    Dir::Column => {
                        if after {
                            vec![(0.25, leaf(3)), (0.25, leaf(1)), (0.5, leaf(4))]
                        } else {
                            vec![(0.25, leaf(1)), (0.25, leaf(3)), (0.5, leaf(4))]
                        }
                    }
                };
                assert_eq!(tree.root(), &row(vec![(0.4, leaf(2)), (0.6, col(target_children))]));
                assert_relocation_invariants(&tree, &[1, 2, 3, 4]);
            }
        }
    }

    #[test]
    fn relocate_to_all_outer_edges_preserves_remaining_nested_layout() {
        for dir in [Dir::Row, Dir::Column] {
            for after in [false, true] {
                let mut tree = relocation_tree();
                assert!(tree.relocate(1, None, dir, after));
                let remaining = row(vec![
                    (0.4, leaf(2)),
                    (0.6, col(vec![(0.5, leaf(3)), (0.5, leaf(4))])),
                ]);
                let children = if after {
                    vec![(0.5, remaining), (0.5, leaf(1))]
                } else {
                    vec![(0.5, leaf(1)), (0.5, remaining)]
                };
                assert_eq!(tree.root(), &Node::Split { dir, children });
                let moved = tree.layout(AREA, 0.0).into_iter().find(|(id, _)| *id == 1).unwrap().1;
                let expected = match (dir, after) {
                    (Dir::Row, false) => Rect::new(0.0, 0.0, AREA.w / 2.0, AREA.h),
                    (Dir::Row, true) => Rect::new(AREA.w / 2.0, 0.0, AREA.w / 2.0, AREA.h),
                    (Dir::Column, false) => Rect::new(0.0, 0.0, AREA.w, AREA.h / 2.0),
                    (Dir::Column, true) => Rect::new(0.0, AREA.h / 2.0, AREA.w, AREA.h / 2.0),
                };
                assert_eq!(moved, expected, "outer edge spans the whole perpendicular axis");
                assert_relocation_invariants(&tree, &[1, 2, 3, 4]);
            }
        }
    }

    #[test]
    fn relocate_within_one_split_redistributes_only_removed_and_target_space() {
        let mut tree = SplitTree::from_root(row(vec![
            (0.5, leaf(1)),
            (0.25, leaf(2)),
            (0.25, leaf(3)),
        ]));
        assert!(tree.relocate(1, Some(3), Dir::Row, true));
        assert_eq!(tree.root(), &row(vec![(0.5, leaf(2)), (0.25, leaf(3)), (0.25, leaf(1))]));
        assert_relocation_invariants(&tree, &[1, 2, 3]);
    }

    #[test]
    fn relocate_rejects_invalid_sources_targets_and_self_without_mutation() {
        for dir in [Dir::Row, Dir::Column] {
            for after in [false, true] {
                let mut tree = relocation_tree();
                let before = tree.clone();
                for (id, target) in [(99, Some(3)), (1, Some(99)), (1, Some(1)), (99, None)] {
                    assert!(!tree.relocate(id, target, dir, after));
                    assert_eq!(tree, before);
                }
                let mut single = SplitTree::new(1);
                for (id, target) in [(1, None), (1, Some(1)), (1, Some(99)), (99, None)] {
                    assert!(!single.relocate(id, target, dir, after));
                    assert_eq!(single.root(), &leaf(1), "last pane is never moved or removed");
                }
            }
        }
    }

    #[test]
    fn repeated_relocation_keeps_normalized_positive_fractions_and_all_ids() {
        let mut tree = relocation_tree();
        for _ in 0..4 {
            for (id, target, dir, after) in [
                (1, Some(4), Dir::Row, false),
                (2, None, Dir::Column, true),
                (3, Some(1), Dir::Column, false),
                (4, None, Dir::Row, true),
            ] {
                assert!(tree.relocate(id, target, dir, after));
                assert_relocation_invariants(&tree, &[1, 2, 3, 4]);
            }
        }
    }

    #[test]
    fn relocate_round_trips_after_nested_and_outer_edge_moves() {
        for target in [Some(3), None] {
            for dir in [Dir::Row, Dir::Column] {
                for after in [false, true] {
                    let mut tree = relocation_tree();
                    assert!(tree.relocate(1, target, dir, after));
                    let json = serde_json::to_string(tree.root()).unwrap();
                    let back: Node = serde_json::from_str(&json).unwrap();
                    assert_eq!(&back, tree.root());
                    let restored = SplitTree::from_root(back);
                    assert_eq!(restored.panes(), tree.panes());
                    assert_relocation_invariants(&restored, &[1, 2, 3, 4]);
                }
            }
        }
    }

    /// A hand-edited session.json with an empty split used to panic on the UI
    /// thread at start (first_leaf indexed children[0]), on every launch.
    #[test]
    fn empty_splits_from_a_session_are_dropped() {
        let empty = || Node::Split { dir: Dir::Column, children: Vec::new() };
        let root = Node::Split {
            dir: Dir::Row,
            children: vec![
                (0.25, empty()),
                (0.25, Node::Leaf(1)),
                (0.25, Node::Leaf(2)),
                (0.25, Node::Split { dir: Dir::Column, children: vec![(1.0, empty())] }),
            ],
        };
        let mut t = SplitTree::from_root(root);
        assert_eq!(t.panes(), vec![1, 2]);
        let anchor = t.remove(1).expect("pane 1 is removed");
        assert_eq!(anchor.neighbor, 2, "its neighbour is a real pane, not an empty split");
        assert_eq!(t.panes(), vec![2]);
    }

    #[test]
    fn split_right_then_down() {
        let mut t = SplitTree::new(1);
        assert!(t.split_right(1, 2));
        assert!(t.split_down(2, 3));
        assert_eq!(t.root(), &row(vec![(0.5, leaf(1)), (0.5, col(vec![(0.5, leaf(2)), (0.5, leaf(3))]))]));
        assert_eq!(t.panes(), vec![1, 2, 3]);
        assert!(!t.split_right(99, 4), "unknown target");
    }

    #[test]
    fn splitting_inside_a_same_direction_split_adds_a_sibling() {
        let mut t = SplitTree::new(1);
        t.split_right(1, 2);
        t.split_right(2, 3);
        assert_eq!(t.root(), &row(vec![(0.5, leaf(1)), (0.25, leaf(2)), (0.25, leaf(3))]));
        t.insert(1, 4, Dir::Row, false);
        assert_eq!(t.panes(), vec![4, 1, 2, 3]);
    }

    #[test]
    fn remove_collapses_and_reports_anchor() {
        let mut t = SplitTree::new(1);
        t.split_right(1, 2);
        t.split_down(2, 3);
        assert_eq!(t.remove(3), Some(Anchor { neighbor: 2, dir: Dir::Column, after: true }));
        assert_eq!(t.root(), &row(vec![(0.5, leaf(1)), (0.5, leaf(2))]));
        assert_eq!(t.remove(1), Some(Anchor { neighbor: 2, dir: Dir::Row, after: false }));
        assert_eq!(t.root(), &leaf(2));
        assert_eq!(t.remove(2), None, "last pane stays");
        assert_eq!(t.remove(42), None);
    }

    #[test]
    fn removed_fraction_goes_to_siblings_proportionally() {
        let mut t = SplitTree::from_root(row(vec![(0.5, leaf(1)), (0.25, leaf(2)), (0.25, leaf(3))]));
        t.remove(1);
        assert_eq!(t.root(), &row(vec![(0.5, leaf(2)), (0.5, leaf(3))]));
    }

    #[test]
    fn restore_returns_pane_to_its_place_or_falls_back() {
        let mut t = SplitTree::from_root(row(vec![(0.4, leaf(1)), (0.3, leaf(2)), (0.3, leaf(3))]));
        let anchor = t.remove(2).unwrap();
        assert!(t.restore(2, anchor, 1));
        assert_eq!(t.panes(), vec![1, 2, 3]);

        let anchor = t.remove(1).unwrap();
        assert_eq!(anchor.neighbor, 2);
        t.remove(2);
        assert!(t.restore(1, anchor, 3));
        assert_eq!(t.panes(), vec![3, 1], "neighbour gone: right of fallback");
    }

    #[test]
    fn layout_and_dividers() {
        let t = SplitTree::from_root(row(vec![(0.5, leaf(1)), (0.5, col(vec![(0.5, leaf(2)), (0.5, leaf(3))]))]));
        let l = t.layout(AREA, GAP);
        assert_eq!(l[0], (1, Rect::new(0.0, 0.0, 150.0, 206.0)));
        assert_eq!(l[1], (2, Rect::new(156.0, 0.0, 150.0, 100.0)));
        assert_eq!(l[2], (3, Rect::new(156.0, 106.0, 150.0, 100.0)));
        let d = t.dividers(AREA, GAP);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0], Divider { path: vec![], index: 0, dir: Dir::Row, rect: Rect::new(150.0, 0.0, 6.0, 206.0) });
        assert_eq!(d[1], Divider { path: vec![1], index: 0, dir: Dir::Column, rect: Rect::new(156.0, 100.0, 150.0, 6.0) });
    }

    #[test]
    fn dragging_respects_min_size() {
        let mut t = SplitTree::from_root(row(vec![(0.5, leaf(1)), (0.5, leaf(2))]));
        let area = Rect::new(0.0, 0.0, 206.0, 100.0);
        let d = t.dividers(area, GAP)[0].clone();
        let close = |a: f32, b: f32| (a - b).abs() < 0.01;
        t.drag_divider(area, GAP, &d, 50.0, 30.0);
        assert!(close(t.layout(area, GAP)[0].1.w, 150.0));
        t.drag_divider(area, GAP, &d, 500.0, 30.0);
        assert!(close(t.layout(area, GAP)[1].1.w, 30.0));
        t.drag_divider(area, GAP, &d, -500.0, 30.0);
        assert!(close(t.layout(area, GAP)[0].1.w, 30.0));
    }

    #[test]
    fn navigate_one_by_three() {
        let t = SplitTree::from_root(row(vec![(1.0, leaf(1)), (1.0, leaf(2)), (1.0, leaf(3))]));
        assert_eq!(t.navigate(2, NavDir::Left, AREA, GAP), Some(1));
        assert_eq!(t.navigate(2, NavDir::Right, AREA, GAP), Some(3));
        assert_eq!(t.navigate(1, NavDir::Left, AREA, GAP), None);
        assert_eq!(t.navigate(2, NavDir::Up, AREA, GAP), None);
    }

    #[test]
    fn navigate_two_by_two() {
        let t = SplitTree::from_root(row(vec![
            (0.5, col(vec![(0.5, leaf(1)), (0.5, leaf(3))])),
            (0.5, col(vec![(0.5, leaf(2)), (0.5, leaf(4))])),
        ]));
        assert_eq!(t.navigate(1, NavDir::Right, AREA, GAP), Some(2));
        assert_eq!(t.navigate(1, NavDir::Down, AREA, GAP), Some(3));
        assert_eq!(t.navigate(4, NavDir::Left, AREA, GAP), Some(3));
        assert_eq!(t.navigate(4, NavDir::Up, AREA, GAP), Some(2));
    }

    #[test]
    fn navigate_t_layout_prefers_reading_order_on_ties() {
        let t = SplitTree::from_root(col(vec![(0.5, row(vec![(0.5, leaf(1)), (0.5, leaf(2))])), (0.5, leaf(3))]));
        assert_eq!(t.navigate(3, NavDir::Up, AREA, GAP), Some(1));
        assert_eq!(t.navigate(2, NavDir::Down, AREA, GAP), Some(3));
        assert_eq!(t.navigate(1, NavDir::Right, AREA, GAP), Some(2));
    }

    #[test]
    fn next_and_previous_cycle() {
        let t = SplitTree::from_root(row(vec![(0.5, leaf(1)), (0.5, col(vec![(0.5, leaf(2)), (0.5, leaf(3))]))]));
        assert_eq!(t.next(3), Some(1));
        assert_eq!(t.previous(1), Some(3));
        assert_eq!(t.next(1), Some(2));
    }

    #[test]
    fn from_root_repairs_saved_state() {
        let t = SplitTree::from_root(row(vec![(2.0, leaf(1)), (2.0, col(vec![(1.0, leaf(2))]))]));
        assert_eq!(t.root(), &row(vec![(0.5, leaf(1)), (0.5, leaf(2))]));
    }

    #[test]
    fn serde_round_trip() {
        let t = SplitTree::from_root(row(vec![(0.5, leaf(1)), (0.5, col(vec![(0.5, leaf(2)), (0.5, leaf(3))]))]));
        let json = serde_json::to_string(t.root()).unwrap();
        let back: Node = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, t.root());
    }
}

//! Commit graph lanes: turns a topologically ordered commit list into the
//! per-row lane geometry a git-style graph is drawn from. Ported from the
//! owner's Helm fork (handles linear history and ordinary merges; deep
//! octopus merges render approximately).

use crate::git::Commit;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Runs from the row top down to the node.
    Up,
    /// Runs from the node down to the row bottom.
    Down,
    /// Spans the whole row.
    Through,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub from_lane: usize,
    pub to_lane: usize,
    pub kind: Kind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub lane: usize,
    pub lane_count: usize,
    pub segments: Vec<Segment>,
}

/// Caps that keep a hostile history (thousands of parents/lanes) from stalling
/// the UI thread; deeper graphs render approximately, as the fork did.
const MAX_LANES: usize = 32;
const MAX_PARENTS: usize = 8;

/// Lane geometry for every commit, in the order they are displayed.
pub fn compute(commits: &[Commit]) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::with_capacity(commits.len());
    // lanes[i] is the hash lane i is currently heading towards.
    let mut lanes: Vec<Option<String>> = Vec::new();

    for commit in commits {
        let mut lane = lanes.iter().position(|hash| hash.as_deref() == Some(commit.hash.as_str()));
        if lane.is_none() {
            lane = Some(lanes.len());
            lanes.push(Some(commit.hash.clone()));
        }
        let lane = lane.expect("lane assigned");

        let incoming = lanes.clone();
        // The node lane continues towards the first parent (or ends).
        let (first_parent, other_parents) = match commit.parents.split_first() {
            Some((first, rest)) => (Some(first.clone()), rest),
            None => (None, &[][..]),
        };
        match first_parent {
            None => lanes[lane] = None,
            Some(parent) => {
                let merged = lanes.iter().position(|hash| hash.as_deref() == Some(parent.as_str()));
                lanes[lane] = if merged == Some(lane) || merged.is_none() { Some(parent) } else { None };
            }
        }
        // Bound the fan-out: an octopus merge with thousands of parents would
        // otherwise stall the UI thread while lanes are cloned per commit.
        for parent in other_parents.iter().take(MAX_PARENTS - 1) {
            if lanes.len() >= MAX_LANES {
                break;
            }
            if !lanes.iter().any(|hash| hash.as_deref() == Some(parent.as_str())) {
                match lanes.iter().position(Option::is_none) {
                    Some(free) => lanes[free] = Some(parent.clone()),
                    None => lanes.push(Some(parent.clone())),
                }
            }
        }
        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }
        let outgoing = lanes.clone();

        let mut segments: Vec<Segment> = Vec::new();
        let push = |segments: &mut Vec<Segment>, segment: Segment| {
            let duplicate = segments
                .iter()
                .any(|s| s.kind == segment.kind && s.from_lane == segment.from_lane && s.to_lane == segment.to_lane);
            if !duplicate {
                segments.push(segment);
            }
        };
        let width = incoming.len().max(outgoing.len()).max(lane + 1);
        for index in 0..width {
            let above = incoming.get(index).and_then(|hash| hash.as_ref());
            let below = outgoing.get(index).and_then(|hash| hash.as_ref());
            if index == lane {
                if above.is_some() {
                    push(&mut segments, Segment { from_lane: index, to_lane: index, kind: Kind::Up });
                }
                continue;
            }
            match (above, below) {
                (Some(above), Some(below)) if above == below => {
                    push(&mut segments, Segment { from_lane: index, to_lane: index, kind: Kind::Through });
                }
                (Some(above), None) => {
                    let target = outgoing
                        .iter()
                        .position(|hash| hash.as_deref() == Some(above.as_str()))
                        .unwrap_or(lane);
                    push(&mut segments, Segment { from_lane: index, to_lane: target, kind: Kind::Up });
                }
                _ => {}
            }
        }
        for parent in &commit.parents {
            if let Some(target) = outgoing.iter().position(|hash| hash.as_deref() == Some(parent.as_str())) {
                push(&mut segments, Segment { from_lane: lane, to_lane: target, kind: Kind::Down });
            }
        }
        // A commit with no drawn parent still needs a stub if history continues.
        if !segments.iter().any(|s| s.kind == Kind::Down) && outgoing.get(lane).is_some_and(|hash| hash.is_some()) {
            push(&mut segments, Segment { from_lane: lane, to_lane: lane, kind: Kind::Down });
        }

        rows.push(Row { lane, lane_count: width.max(outgoing.len()), segments });
    }

    // Every row reserves the same width as its widest neighbour.
    let max_lanes = rows.iter().map(|row| row.lane_count).max().unwrap_or(1).max(1);
    for row in &mut rows {
        row.lane_count = max_lanes;
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::Section;

    fn commit(hash: &str, parents: &[&str], section: Section) -> Commit {
        Commit {
            hash: hash.to_owned(),
            short: hash[..7.min(hash.len())].to_owned(),
            parents: parents.iter().map(|p| (*p).to_owned()).collect(),
            author: "a".into(),
            subject: "s".into(),
            refs: Vec::new(),
            time: 0,
            section,
        }
    }

    #[test]
    fn linear_history_is_one_lane() {
        let commits = vec![
            commit("ccccccc1", &["bbbbbbb1"], Section::Outgoing),
            commit("bbbbbbb1", &["aaaaaaa1"], Section::Outgoing),
            commit("aaaaaaa1", &[], Section::History),
        ];
        let rows = compute(&commits);
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|row| row.lane == 0 && row.lane_count == 1));
        // The node lane draws a line from the row top to the node and on to the
        // row bottom (up + down), exactly as the fork's algorithm emits it.
        assert_eq!(
            rows[0].segments,
            vec![
                Segment { from_lane: 0, to_lane: 0, kind: Kind::Up },
                Segment { from_lane: 0, to_lane: 0, kind: Kind::Down },
            ]
        );
        // The last commit ends history: a stub from the top to the node only.
        assert_eq!(rows[2].segments, vec![Segment { from_lane: 0, to_lane: 0, kind: Kind::Up }]);
    }

    #[test]
    fn merge_opens_a_second_lane() {
        // m merges a and b; a and b share the root.
        let commits = vec![
            commit("mmmmmmm1", &["aaaaaaa1", "bbbbbbb1"], Section::Outgoing),
            commit("aaaaaaa1", &["rrrrrrr1"], Section::Outgoing),
            commit("bbbbbbb1", &["rrrrrrr1"], Section::Incoming),
            commit("rrrrrrr1", &[], Section::History),
        ];
        let rows = compute(&commits);
        assert_eq!(rows[0].lane, 0);
        assert_eq!(rows[0].lane_count, 2, "the merge needs two lanes");
        assert!(
            rows[0].segments.iter().any(|s| s.kind == Kind::Down && s.from_lane == 0 && s.to_lane == 1),
            "merge edge into the second lane: {:?}",
            rows[0].segments
        );
        assert_eq!(rows[1].lane, 0);
        assert_eq!(rows[2].lane, 1, "the side branch keeps its lane");
        assert!(rows[3].segments.iter().any(|s| s.kind == Kind::Through || s.kind == Kind::Up));
    }

    #[test]
    fn second_parent_reuses_a_free_lane() {
        let commits = vec![
            commit("mmmmmmm1", &["aaaaaaa1", "bbbbbbb1"], Section::Outgoing),
            commit("aaaaaaa1", &["rrrrrrr1"], Section::Outgoing),
            commit("bbbbbbb1", &["sssssss1"], Section::Outgoing),
            commit("rrrrrrr1", &[], Section::History),
            commit("sssssss1", &[], Section::History),
        ];
        let rows = compute(&commits);
        assert!(rows.iter().all(|row| row.lane_count >= 2));
        assert_eq!(rows[4].lane, 1);
    }

    #[test]
    fn no_duplicate_segments() {
        let commits = vec![
            commit("ccccccc1", &["bbbbbbb1"], Section::Outgoing),
            commit("bbbbbbb1", &["aaaaaaa1"], Section::Outgoing),
            commit("aaaaaaa1", &[], Section::History),
        ];
        for row in compute(&commits) {
            let mut seen = std::collections::HashSet::new();
            for segment in &row.segments {
                assert!(seen.insert((segment.kind, segment.from_lane, segment.to_lane)), "duplicate in {row:?}");
            }
        }
    }
}

//! Lays out the lines of the commit graph, one row per commit.
//!
//! Each branch of history runs down its own lane. A row says which lane its
//! commit sits in and which lines cross it: from the top edge down to the
//! middle, and from the middle down to the bottom edge. Drawing every row's
//! lines gives the whole graph, with no row needing to know about another.

use crate::git::Commit;

/// One commit's slice of the graph.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    /// The lane the commit's dot is in.
    pub lane: usize,
    /// Lines from (lane at the top edge) to (lane at the middle).
    pub top: Vec<(usize, usize)>,
    /// Lines from (lane at the middle) to (lane at the bottom edge).
    pub bottom: Vec<(usize, usize)>,
}

impl Row {
    /// How many lanes this row uses.
    pub fn width(&self) -> usize {
        self.top
            .iter()
            .chain(&self.bottom)
            .flat_map(|(a, b)| [*a, *b])
            .chain([self.lane])
            .max()
            .map_or(1, |m| m + 1)
    }
}

pub fn layout(commits: &[Commit]) -> Vec<Row> {
    // What each lane is waiting for: the commit it leads to next.
    let mut lanes: Vec<Option<&str>> = Vec::new();
    let mut rows = Vec::with_capacity(commits.len());
    for commit in commits {
        let hash = commit.hash.as_str();
        let lane = lanes
            .iter()
            .position(|l| *l == Some(hash))
            .or_else(|| lanes.iter().position(Option::is_none))
            .unwrap_or_else(|| {
                lanes.push(None);
                lanes.len() - 1
            });
        let mut row = Row {
            lane,
            ..Row::default()
        };
        for (i, waiting) in lanes.iter_mut().enumerate() {
            match *waiting {
                // Every lane that led here ends at this commit.
                Some(h) if h == hash => {
                    row.top.push((i, lane));
                    *waiting = None;
                }
                Some(_) => row.top.push((i, i)),
                None => {}
            }
        }
        // Lanes passing straight through carry on below.
        for (i, waiting) in lanes.iter().enumerate() {
            if waiting.is_some() {
                row.bottom.push((i, i));
            }
        }
        // The first parent continues this lane; others join or open lanes.
        for (n, parent) in commit.parents.iter().enumerate() {
            let parent = parent.as_str();
            let target = if n == 0 {
                lane
            } else if let Some(existing) = lanes.iter().position(|l| *l == Some(parent)) {
                row.bottom.push((lane, existing));
                continue;
            } else {
                lanes.iter().position(Option::is_none).unwrap_or_else(|| {
                    lanes.push(None);
                    lanes.len() - 1
                })
            };
            lanes[target] = Some(parent);
            row.bottom.push((lane, target));
        }
        while lanes.last() == Some(&None) {
            lanes.pop();
        }
        rows.push(row);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(hash: &str, parents: &[&str]) -> Commit {
        Commit {
            hash: hash.to_owned(),
            short: hash.to_owned(),
            parents: parents.iter().map(|p| (*p).to_owned()).collect(),
            author: String::new(),
            time: 0,
            subject: String::new(),
            refs: Vec::new(),
            incoming: false,
        }
    }

    #[test]
    fn a_straight_line_stays_in_one_lane() {
        let rows = layout(&[commit("c", &["b"]), commit("b", &["a"]), commit("a", &[])]);
        assert_eq!(rows[0].lane, 0);
        assert_eq!(rows[0].top, []);
        assert_eq!(rows[0].bottom, [(0, 0)]);
        assert_eq!(rows[1].top, [(0, 0)]);
        assert_eq!(rows[1].bottom, [(0, 0)]);
        // The first commit has nothing below it.
        assert_eq!(rows[2].bottom, []);
        assert!(rows.iter().all(|r| r.width() == 1));
    }

    #[test]
    fn a_merge_opens_a_lane_that_closes_where_the_branches_split() {
        // m merges f into d; both came from a.
        let rows = layout(&[
            commit("m", &["d", "f"]),
            commit("f", &["a"]),
            commit("d", &["a"]),
            commit("a", &[]),
        ]);
        // The merge: first parent straight down, second parent in a new lane.
        assert_eq!(rows[0].bottom, [(0, 0), (0, 1)]);
        // f sits in that second lane while the first passes by.
        assert_eq!(rows[1].lane, 1);
        assert_eq!(rows[1].top, [(0, 0), (1, 1)]);
        assert_eq!(rows[1].bottom, [(0, 0), (1, 1)]);
        assert_eq!(rows[2].lane, 0);
        // Both lanes end at a.
        assert_eq!(rows[3].lane, 0);
        assert_eq!(rows[3].top, [(0, 0), (1, 0)]);
        assert_eq!(rows.iter().map(Row::width).max(), Some(2));
    }

    #[test]
    fn a_second_tip_starts_its_own_lane() {
        // Two branch tips, r (incoming) and l (local), sharing parent a.
        let rows = layout(&[commit("r", &["a"]), commit("l", &["a"]), commit("a", &[])]);
        assert_eq!(rows[1].lane, 1);
        assert_eq!(rows[1].top, [(0, 0)]);
        assert_eq!(rows[2].top, [(0, 0), (1, 0)]);
    }
}

//! The commit graph's geometry: which lane each commit sits in, and the
//! lines that join it to its parents, row by row.
//!
//! Lanes are columns. A lane holds the commit it expects next, so a branch
//! runs down one lane until that commit arrives. Lanes never shift sideways:
//! a line that passes a row stays straight, and only lines that meet a
//! commit bend.

/// Where in its row a line starts or ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Height {
    Top,
    /// The commit's dot.
    Middle,
    Bottom,
}

/// A line within one row, from one lane and height to another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Edge {
    pub(crate) from: (usize, Height),
    pub(crate) to: (usize, Height),
    /// Index into the lane palette.
    pub(crate) color: usize,
}

/// One commit's row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GraphRow {
    pub(crate) lane: usize,
    pub(crate) color: usize,
    pub(crate) edges: Vec<Edge>,
}

impl GraphRow {
    /// How many lanes the row touches.
    pub(crate) fn width(&self) -> usize {
        self.edges
            .iter()
            .flat_map(|edge| [edge.from.0, edge.to.0])
            .chain([self.lane])
            .max()
            .map_or(1, |lane| lane + 1)
    }
}

/// Lay out commits given children before parents, as `(id, parents)`.
/// A parent missing from the list, past the history's end, keeps its lane
/// running to the bottom.
pub(crate) fn layout<'a>(
    commits: impl IntoIterator<Item = (&'a str, &'a [String])>,
) -> Vec<GraphRow> {
    // Each lane: the commit it waits for, and its color.
    let mut lanes: Vec<Option<(&str, usize)>> = Vec::new();
    let mut next_color = 0;
    let mut new_color = || {
        let color = next_color;
        next_color += 1;
        color
    };
    let mut rows = Vec::new();

    for (id, parents) in commits {
        let waiting: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter(|(_, lane)| lane.is_some_and(|(expected, _)| expected == id))
            .map(|(ix, _)| ix)
            .collect();
        let (lane, color) = match waiting.first() {
            Some(&ix) => (ix, lanes[ix].map_or(0, |(_, color)| color)),
            None => (free_lane(&mut lanes), new_color()),
        };

        let mut edges = Vec::new();
        // Into the row: lines meeting this commit bend to its dot; the rest
        // pass straight through.
        for (ix, slot) in lanes.iter().enumerate() {
            if let Some((expected, lane_color)) = *slot {
                if expected == id {
                    edges.push(Edge {
                        from: (ix, Height::Top),
                        to: (lane, Height::Middle),
                        color: lane_color,
                    });
                } else {
                    edges.push(Edge {
                        from: (ix, Height::Top),
                        to: (ix, Height::Bottom),
                        color: lane_color,
                    });
                }
            }
        }
        for ix in waiting {
            lanes[ix] = None;
        }

        // Out of the row, to each parent. The first parent continues this
        // lane, even when another lane waits for it too: the two run side by
        // side and meet at the fork, as `git log --graph` draws them. A
        // merged parent joins a lane already waiting for it, or opens one.
        for (nth, parent) in parents.iter().enumerate() {
            let parent = parent.as_str();
            if nth == 0 {
                lanes[lane] = Some((parent, color));
                edges.push(Edge {
                    from: (lane, Height::Middle),
                    to: (lane, Height::Bottom),
                    color,
                });
                continue;
            }
            let existing = lanes
                .iter()
                .position(|slot| slot.is_some_and(|(expected, _)| expected == parent));
            let (target, edge_color) = match existing {
                // The merge line takes the color of the branch it comes from.
                Some(ix) => (ix, lanes[ix].map_or(0, |(_, color)| color)),
                None => {
                    let ix = free_lane(&mut lanes);
                    let color = new_color();
                    lanes[ix] = Some((parent, color));
                    (ix, color)
                }
            };
            edges.push(Edge {
                from: (lane, Height::Middle),
                to: (target, Height::Bottom),
                color: edge_color,
            });
        }

        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }
        rows.push(GraphRow { lane, color, edges });
    }
    rows
}

/// The leftmost empty lane, made if there is none.
fn free_lane(lanes: &mut Vec<Option<(&str, usize)>>) -> usize {
    match lanes.iter().position(Option::is_none) {
        Some(ix) => ix,
        None => {
            lanes.push(None);
            lanes.len() - 1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(commits: &[(&str, &[&str])]) -> Vec<GraphRow> {
        let owned: Vec<(String, Vec<String>)> = commits
            .iter()
            .map(|(id, parents)| {
                (
                    id.to_string(),
                    parents.iter().map(|parent| parent.to_string()).collect(),
                )
            })
            .collect();
        layout(
            owned
                .iter()
                .map(|(id, parents)| (id.as_str(), parents.as_slice())),
        )
    }

    #[test]
    fn keeps_a_straight_history_in_one_lane() {
        let rows = rows(&[("c", &["b"]), ("b", &["a"]), ("a", &[])]);
        assert!(rows.iter().all(|row| row.lane == 0 && row.width() == 1));
        assert!(rows.iter().all(|row| row.color == 0));
        // The root ends its lane: a line comes in, none goes out.
        assert_eq!(
            rows[2].edges,
            [Edge {
                from: (0, Height::Top),
                to: (0, Height::Middle),
                color: 0
            }]
        );
    }

    #[test]
    fn opens_a_lane_for_a_merged_branch_and_closes_it_at_the_fork() {
        // m merges s into main; s and b both come from a.
        let rows = rows(&[("m", &["b", "s"]), ("s", &["a"]), ("b", &["a"]), ("a", &[])]);
        assert_eq!(rows[0].lane, 0);
        assert_eq!(rows[0].width(), 2);
        // The side branch sits in its own lane, in its own color.
        assert_eq!(rows[1].lane, 1);
        assert_ne!(rows[1].color, rows[0].color);
        // Main passes the side commit straight.
        assert!(rows[1].edges.contains(&Edge {
            from: (0, Height::Top),
            to: (0, Height::Bottom),
            color: 0
        }));
        assert_eq!(rows[2].lane, 0);
        // At the fork both lanes bend into the one dot.
        assert_eq!(rows[3].lane, 0);
        let incoming: Vec<_> = rows[3]
            .edges
            .iter()
            .filter(|edge| edge.to == (0, Height::Middle))
            .map(|edge| edge.from.0)
            .collect();
        assert_eq!(incoming, [0, 1]);
    }

    #[test]
    fn runs_a_lane_past_the_end_of_a_short_history() {
        let rows = rows(&[("b", &["a"])]);
        assert_eq!(
            rows[0].edges,
            [Edge {
                from: (0, Height::Middle),
                to: (0, Height::Bottom),
                color: 0
            }]
        );
    }

    #[test]
    fn places_unrelated_branch_tips_side_by_side() {
        let rows = rows(&[("x", &["a"]), ("y", &["b"]), ("a", &[]), ("b", &[])]);
        assert_eq!((rows[0].lane, rows[1].lane), (0, 1));
        assert_eq!((rows[2].lane, rows[3].lane), (0, 1));
    }
}

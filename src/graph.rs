use crate::{Commit, Rgb};

/// A straight line segment within one commit row. `x` is in lane units
/// (lane centres are at integers), `y` is 0 at the top of the row and 1 at
/// the bottom; the node sits at y = 0.5.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seg {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub color: Rgb,
}

/// gitk's default line palette.
pub const PALETTE: [Rgb; 7] = [
    [0x00, 0xff, 0x00],
    [0xff, 0x00, 0x00],
    [0x00, 0x00, 0xff],
    [0xff, 0x00, 0xff],
    [0xa9, 0xa9, 0xa9],
    [0xa5, 0x2a, 0x2a],
    [0xff, 0xa5, 0x00],
];

/// Rows between two snapshots in `Lanes::snapshots`; `segs` replays at most this many.
const EVERY: usize = 32;

/// The lanes between two rows: (row of the commit each lane leads to,
/// palette index), plus the round-robin colour state.
#[derive(Clone, Default)]
pub(crate) struct Lanes {
    lanes: Vec<(u32, u8)>,
    next_color: u8,
}

/// How one row changes the lanes.
struct Step {
    node: usize,
    /// Lane count at mid-row, including a lane opened for this commit.
    mid: usize,
    /// For each lane below, the mid-row lane it comes from.
    src: Vec<usize>,
    /// Lanes below that the node merges into besides its own.
    merges: Vec<usize>,
}

impl Lanes {
    fn pick(&mut self, avoid: &[u8]) -> u8 {
        // round robin, skipping the neighbours' colours
        for _ in 0..PALETTE.len() {
            let c = self.next_color;
            self.next_color = (self.next_color + 1) % PALETTE.len() as u8;
            if !avoid.contains(&c) {
                return c;
            }
        }
        self.next_color
    }

    /// Moves past `row`, whose parents are given as rows (None when not in
    /// the view: such parents get no lane, like gitk's terminated edges).
    fn step(&mut self, row: usize, parents: &[Option<usize>]) -> Step {
        let row = row as u32;
        let node = match self.lanes.iter().position(|l| l.0 == row) {
            Some(i) => i,
            None => {
                let avoid: Vec<u8> = self.lanes.last().map(|l| l.1).into_iter().collect();
                let color = self.pick(&avoid);
                self.lanes.push((row, color));
                self.lanes.len() - 1
            }
        };
        let mid = std::mem::take(&mut self.lanes);
        let mut after: Vec<(u32, u8)> = vec![];
        let mut src = vec![];
        for (i, &l) in mid.iter().enumerate() {
            if i != node {
                after.push(l);
                src.push(i);
                continue;
            }
            for (k, &p) in parents.iter().enumerate() {
                let Some(p) = p else { continue };
                let p = p as u32;
                if mid.iter().chain(&after).any(|m| m.0 == p) {
                    continue;
                }
                let color = if k == 0 {
                    l.1
                } else {
                    let avoid: Vec<u8> = after.iter().map(|a| a.1).chain([l.1]).collect();
                    self.pick(&avoid)
                };
                after.push((p, color));
                src.push(node);
            }
        }
        let merges = parents
            .iter()
            .flatten()
            .filter_map(|&p| (0..after.len()).find(|&j| after[j].0 == p as u32 && src[j] != node))
            .collect();
        self.lanes = after;
        Step { node, mid: mid.len(), src, merges }
    }
}

fn parent_rows(c: &Commit, rows: &impl Fn(&str) -> Option<usize>) -> Vec<Option<usize>> {
    c.parents().map(rows).collect()
}

/// Assigns lanes; returns each row's (node lane, width) and lane snapshots
/// every `EVERY` rows for `segs`. Lanes are compacted every row, so lines
/// shift sideways when a lane to their left ends.
// No gitk-style arrow breaking of long edges: many long-lived branches make
// the graph wide. Add it if that matters.
pub(crate) fn layout(
    commits: &[Commit],
    rows: impl Fn(&str) -> Option<usize>,
) -> (Vec<(usize, usize)>, Vec<Lanes>) {
    let mut lanes = Lanes::default();
    let mut snapshots = vec![];
    let mut placed = Vec::with_capacity(commits.len());
    for (row, c) in commits.iter().enumerate() {
        if row % EVERY == 0 {
            snapshots.push(lanes.clone());
        }
        let s = lanes.step(row, &parent_rows(c, &rows));
        placed.push((s.node, s.mid.max(s.src.len())));
    }
    (placed, snapshots)
}

/// Line segments of `row`, replayed from the nearest snapshot. Lane targets
/// are unique, so every line from the row above runs straight down.
pub(crate) fn segs(
    commits: &[Commit],
    snapshots: &[Lanes],
    row: usize,
    rows: impl Fn(&str) -> Option<usize>,
) -> Vec<Seg> {
    let first = row / EVERY * EVERY;
    let mut lanes = snapshots[row / EVERY].clone();
    for r in first..row {
        lanes.step(r, &parent_rows(&commits[r], &rows));
    }
    let seg = |x0: usize, y0, x1: usize, y1, color: u8| Seg {
        x0: x0 as f32,
        y0,
        x1: x1 as f32,
        y1,
        color: PALETTE[color as usize],
    };
    let mut out: Vec<Seg> =
        lanes.lanes.iter().enumerate().map(|(i, l)| seg(i, 0.0, i, 0.5, l.1)).collect();
    let s = lanes.step(row, &parent_rows(&commits[row], &rows));
    let color = |j: usize| lanes.lanes[j].1;
    out.extend(s.src.iter().enumerate().map(|(j, &src)| seg(src, 0.5, j, 1.0, color(j))));
    out.extend(s.merges.iter().map(|&j| seg(s.node, 0.5, j, 1.0, color(j))));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(id: &str, parents: &[&str]) -> Commit {
        Commit::new(id, &parents.join(" "), "", 0, "", 0, "")
    }

    /// Lays out `cs`, setting lane and width, and returns each row's segments.
    fn lay(cs: &mut [Commit]) -> Vec<Vec<Seg>> {
        let rows = |cs: &[Commit], id: &str| cs.iter().position(|c| c.id() == id);
        let (placed, snaps) = layout(cs, |id| rows(cs, id));
        for (c, (lane, width)) in cs.iter_mut().zip(placed) {
            (c.lane, c.width) = (lane, width);
        }
        (0..cs.len()).map(|r| segs(cs, &snaps, r, |id| rows(cs, id))).collect()
    }

    fn has(segs: &[Seg], x0: f32, y0: f32, x1: f32, y1: f32) -> bool {
        segs.iter().any(|s| (s.x0, s.y0, s.x1, s.y1) == (x0, y0, x1, y1))
    }

    #[test]
    fn merge_and_fork() {
        //   m        merge of a and b
        //   |\
        //   a b
        //   |/
        //   r
        let mut cs = vec![
            commit("m", &["a", "b"]),
            commit("a", &["r"]),
            commit("b", &["r"]),
            commit("r", &[]),
        ];
        let sg = lay(&mut cs);
        assert_eq!(cs.iter().map(|c| c.lane).collect::<Vec<_>>(), [0, 0, 1, 0]);
        // m: no line above, two lines going down
        assert!(has(&sg[0], 0.0, 0.5, 0.0, 1.0) && has(&sg[0], 0.0, 0.5, 1.0, 1.0));
        assert!(!sg[0].iter().any(|s| s.y0 == 0.0));
        // a: lane 1 passes by
        assert!(has(&sg[1], 1.0, 0.0, 1.0, 0.5) && has(&sg[1], 1.0, 0.5, 1.0, 1.0));
        // b: joins r's lane 0 below
        assert!(has(&sg[2], 1.0, 0.5, 0.0, 1.0));
        // r: root, only lines from above
        assert!(sg[3].iter().all(|s| s.y1 == 0.5));
        assert_eq!(sg[3].len(), 1);
        // a and its parent line keep m's first-parent colour
        assert_eq!(sg[0][0].color, sg[1][0].color);
    }

    #[test]
    fn missing_parents_end() {
        // a filtered view: no listed commit is another's parent
        let mut cs: Vec<Commit> =
            (0..50).map(|i| commit(&format!("c{i}"), &[&format!("hidden{i}")])).collect();
        let sg = lay(&mut cs);
        assert!(cs.iter().all(|c| c.width == 1 && c.lane == 0));
        assert!(sg.iter().all(Vec::is_empty));
    }

    #[test]
    fn lanes_compact() {
        // x is a tip on its own lane next to y's line; after x ends, lane 1 shifts left
        let mut cs =
            vec![commit("t", &["p"]), commit("x", &["q"]), commit("p", &["q"]), commit("q", &[])];
        let sg = lay(&mut cs);
        assert_eq!(cs[1].lane, 1);
        assert_eq!(cs[2].lane, 0);
        assert!(has(&sg[2], 0.0, 0.5, 0.0, 1.0)); // p's lane continues to q
        assert!(has(&sg[2], 1.0, 0.0, 1.0, 0.5)); // x's line passes p's row
        assert!(has(&sg[2], 1.0, 0.5, 0.0, 1.0) || has(&sg[3], 1.0, 0.0, 0.0, 0.5));
    }
}

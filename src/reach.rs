//! Every commit reachable from any ref, so "which branches contain X" and
//! "which tag comes after X" are walks in memory: `git for-each-ref
//! --contains` and `git name-rev` walk the history on every call when the
//! repository has no commit-graph file.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::{git, id_key};

pub(crate) struct Reach {
    /// (id key, index), sorted. Lookups trust the 64-bit key alone; a
    /// collision among a repo's commits is vanishingly unlikely.
    keys: Vec<(u64, u32)>,
    /// Children of commit i are `children[start[i]..start[i + 1]]`.
    start: Vec<u32>,
    children: Vec<u32>,
    /// Parents of commit i, in order, are `parents[pstart[i]..pstart[i + 1]]`.
    pstart: Vec<u32>,
    parents: Vec<u32>,
    /// Committer dates.
    dates: Vec<i64>,
    /// Branch tips: (index, short name), in refname order.
    tips: Vec<(u32, String)>,
    /// Tags on commits: (index, tagger date, short name), oldest first, then
    /// by refname. Lightweight tags use the commit's committer date, as
    /// name-rev does.
    tags: Vec<(u32, i64, String)>,
    /// The tag git describe names each tagged commit by: (annotated,
    /// short name). Lightweight tags count as git describe --tags does.
    described: HashMap<u32, (bool, String)>,
    /// Distinct objects tags point at; describe stops looking once it has
    /// found that many.
    tagged_objects: usize,
    /// With a commit-graph git name-rev cuts off by generation instead of
    /// date, and is fast anyway; `precedes` leaves those repos to git.
    has_commit_graph: bool,
}

/// git name-rev's cost of stepping to a second or later parent.
const MERGE_WEIGHT: i64 = 65535;
/// git name-rev ignores commits this much older than the one being named.
const CUTOFF_SLOP: i64 = 86400;

/// A commit's name while replaying name-rev: the tag it is named after.
#[derive(Clone, Copy)]
struct Name {
    tag: u32,
    generation: i64,
    distance: i64,
}

impl Name {
    fn effective(&self) -> i64 {
        self.distance + if self.generation > 0 { MERGE_WEIGHT } else { 0 }
    }
}

impl Reach {
    pub(crate) fn read(cwd: &PathBuf) -> Option<Reach> {
        let list = git(cwd, &["rev-list", "--all", "--parents", "--timestamp"]).ok()?;
        let refs = git(
            cwd,
            &[
                "for-each-ref",
                "--format=%(objectname) %(*objectname) %(taggerdate:unix) %(committerdate:unix) %(refname)",
                "refs/heads",
                "refs/remotes",
                "refs/tags",
            ],
        )
        .ok()?;
        // "<committer time> <id> <parents>..."
        let mut dates = vec![];
        let lines: Vec<Vec<u64>> = list
            .lines()
            .map(|l| {
                let (t, ids) = l.split_once(' ').unwrap_or(("0", l));
                dates.push(t.parse().unwrap_or(0));
                ids.split(' ').filter_map(id_key).collect()
            })
            .collect();
        let mut keys: Vec<(u64, u32)> = lines.iter().enumerate().filter_map(|(i, l)| Some((*l.first()?, i as u32))).collect();
        keys.sort_unstable();
        let git_path = |p: &str| git(cwd, &["rev-parse", "--git-path", p]).map(|s| cwd.join(s.trim()));
        let has_commit_graph = ["objects/info/commit-graph", "objects/info/commit-graphs/commit-graph-chain"]
            .iter()
            .any(|p| git_path(p).is_ok_and(|p| Path::new(&p).exists()));
        let mut r = Reach {
            keys,
            start: vec![],
            children: vec![],
            pstart: vec![0],
            parents: vec![],
            dates,
            tips: vec![],
            tags: vec![],
            described: HashMap::new(),
            tagged_objects: 0,
            has_commit_graph,
        };

        let mut edges: Vec<(u32, u32)> = vec![];
        for (child, l) in lines.iter().enumerate() {
            for &p in &l[1..] {
                if let Some(p) = r.index(p) {
                    edges.push((p, child as u32));
                    r.parents.push(p);
                }
            }
            r.pstart.push(r.parents.len() as u32);
        }
        edges.sort_unstable();
        r.start = vec![0; lines.len() + 1];
        for &(p, _) in &edges {
            r.start[p as usize + 1] += 1;
        }
        for i in 0..lines.len() {
            r.start[i + 1] += r.start[i];
        }
        r.children = edges.into_iter().map(|(_, c)| c).collect();

        let mut tagged = HashSet::new();
        // for describe: (annotated, tagger date) of the name kept per commit
        let mut kept: HashMap<u32, (bool, i64)> = HashMap::new();
        for l in refs.lines() {
            let f: Vec<&str> = l.splitn(5, ' ').collect();
            let [id, peeled, tag_date, commit_date, name] = f[..] else { continue };
            if let Some(t) = name.strip_prefix("refs/tags/") {
                // an annotated tag without a tagger line has date 0 in git
                let annotated = !peeled.is_empty();
                let (id, date) = if annotated { (peeled, tag_date) } else { (id, commit_date) };
                let date = date.parse().unwrap_or(0);
                tagged.insert(id);
                if let Some(i) = id_key(id).and_then(|k| r.index(k)) {
                    r.tags.push((i, date, t.to_string()));
                    // annotated beats lightweight, then the newer annotated
                    // tag; otherwise the first in refname order stays
                    let replace = match kept.get(&i) {
                        None => true,
                        Some(&(a, d)) => (!a && annotated) || (a && annotated && d < date),
                    };
                    if replace {
                        kept.insert(i, (annotated, date));
                        r.described.insert(i, (annotated, t.to_string()));
                    }
                }
                continue;
            }
            if name.ends_with("/HEAD") {
                continue;
            }
            let short = name.strip_prefix("refs/heads/").unwrap_or_else(|| name.trim_start_matches("refs/"));
            if let Some(i) = id_key(id).and_then(|k| r.index(k)) {
                r.tips.push((i, short.to_string()));
            }
        }
        // name-rev tries older tags first; ties keep refname order here
        r.tags.sort_by_key(|t| t.1);
        r.tagged_objects = tagged.len();
        Some(r)
    }

    fn index(&self, key: u64) -> Option<u32> {
        let i = self.keys.binary_search_by_key(&key, |&(k, _)| k).ok()?;
        Some(self.keys[i].1)
    }

    /// Branches whose tip descends from `id` (or is it), like
    /// `git for-each-ref --contains`; None when `id` is not reachable from any ref.
    pub(crate) fn branches_containing(&self, id: &str) -> Option<Vec<&str>> {
        let from = self.index(id_key(id)?)?;
        let mut seen = vec![false; self.start.len() - 1];
        seen[from as usize] = true;
        let mut todo = vec![from];
        while let Some(i) = todo.pop() {
            let i = i as usize;
            for &c in &self.children[self.start[i] as usize..self.start[i + 1] as usize] {
                if !std::mem::replace(&mut seen[c as usize], true) {
                    todo.push(c);
                }
            }
        }
        Some(self.tips.iter().filter(|(i, _)| seen[*i as usize]).map(|(_, n)| n.as_str()).collect())
    }

    /// The tag `git name-rev --tags --no-undefined` names `id` after; empty
    /// when no tag follows it. None when `id` is not reachable from any ref
    /// or the repo has a commit-graph.
    ///
    /// Replays name-rev (builtin/name-rev.c, git 2.56): each tag, oldest
    /// first, names its ancestors depth first, first parent first, and a
    /// commit takes a new name only when it is strictly nearer (a step to a
    /// second or later parent counts 65535). Only descendants of `id` can
    /// affect its name, so only they are walked; like git, commits older
    /// than `id` by more than a day are skipped.
    pub(crate) fn precedes(&self, id: &str) -> Option<&str> {
        if self.has_commit_graph {
            return None;
        }
        let from = self.index(id_key(id)?)? as usize;
        let cutoff = self.dates[from] - CUTOFF_SLOP;
        let n = self.dates.len();
        let mut desc = vec![false; n];
        desc[from] = true;
        let mut todo = vec![from as u32];
        while let Some(i) = todo.pop() {
            let i = i as usize;
            for &c in &self.children[self.start[i] as usize..self.start[i + 1] as usize] {
                if self.dates[c as usize] >= cutoff && !std::mem::replace(&mut desc[c as usize], true) {
                    todo.push(c);
                }
            }
        }

        let mut names: Vec<Option<Name>> = vec![None; n];
        let update = |names: &mut [Option<Name>], i: usize, new: Name| {
            if names[i].is_some_and(|old| old.effective() <= new.effective()) {
                return false;
            }
            names[i] = Some(new);
            true
        };
        for (t, &(tip, _, _)) in self.tags.iter().enumerate() {
            let tip = tip as usize;
            if !desc[tip] || !update(&mut names, tip, Name { tag: t as u32, generation: 0, distance: 0 }) {
                continue;
            }
            let mut stack = vec![tip];
            let mut queue = vec![];
            while let Some(c) = stack.pop() {
                let name = names[c].expect("named before it is stacked");
                queue.clear();
                for (k, &p) in self.parents[self.pstart[c] as usize..self.pstart[c + 1] as usize].iter().enumerate() {
                    let p = p as usize;
                    if !desc[p] {
                        continue;
                    }
                    let (generation, distance) =
                        if k > 0 { (0, name.distance + MERGE_WEIGHT) } else { (name.generation + 1, name.distance + 1) };
                    if update(&mut names, p, Name { tag: name.tag, generation, distance }) {
                        queue.push(p);
                    }
                }
                // the first parent comes off the stack first
                stack.extend(queue.drain(..).rev());
            }
        }
        Some(names[from].map_or("", |n| self.tags[n.tag as usize].2.as_str()))
    }

    /// Parents of `id` in order; None when `id` is not reachable from any ref.
    pub(crate) fn parents_of(&self, id: &str) -> Option<Vec<u32>> {
        let i = self.index(id_key(id)?)? as usize;
        Some(self.parents[self.pstart[i] as usize..self.pstart[i + 1] as usize].to_vec())
    }

    /// The tag `git describe --tags --abbrev=0` gives for commit `c`.
    ///
    /// Replays describe (builtin/describe.c, git 2.56): a tag on `c` itself
    /// wins; otherwise ancestors are walked newest committer date first
    /// (ties in queue order), collecting up to 10 tagged commits. Each
    /// candidate's depth counts walked commits it does not reach; the
    /// shallowest, then the first found, wins. The walk stops early once
    /// an annotated tag has been found and the remaining path is covered by
    /// the best candidates.
    // Tags whose object names differ from their ref ("misnamed") get a
    // suffix from git; this gives the ref name alone.
    pub(crate) fn describe(&self, c: u32) -> Option<&str> {
        const SEEN: u32 = 1;
        const MAX_CANDIDATES: usize = 10;
        if let Some((_, name)) = self.described.get(&c) {
            return Some(name);
        }
        struct Candidate<'a> {
            name: &'a str,
            depth: usize,
            flag: u32,
        }
        let mut flags = vec![0u32; self.dates.len()];
        let mut queue = BinaryHeap::new();
        let mut order = 0u64;
        flags[c as usize] = SEEN;
        queue.push((self.dates[c as usize], Reverse(order), c));
        let mut found: Vec<Candidate> = vec![];
        let mut annotated = 0;
        let mut seen = 0;
        while let Some((_, _, c)) = queue.pop() {
            let c = c as usize;
            seen += 1;
            if found.len() == MAX_CANDIDATES || found.len() == self.tagged_objects {
                break;
            }
            if let Some((is_annotated, name)) = self.described.get(&(c as u32)) {
                let flag = 1 << (found.len() + 1);
                found.push(Candidate { name, depth: seen - 1, flag });
                flags[c] |= flag;
                annotated += *is_annotated as usize;
            }
            for t in &mut found {
                if flags[c] & t.flag == 0 {
                    t.depth += 1;
                }
            }
            if annotated > 0 && queue.is_empty() {
                let best = found.iter().map(|t| t.depth).min().expect("annotated tags are in found");
                let within = found.iter().filter(|t| t.depth == best).fold(0, |w, t| w | t.flag);
                if flags[c] & within == within {
                    break;
                }
            }
            for &p in &self.parents[self.pstart[c] as usize..self.pstart[c + 1] as usize] {
                let p = p as usize;
                if flags[p] & SEEN == 0 {
                    order += 1;
                    queue.push((self.dates[p], Reverse(order), p as u32));
                }
                flags[p] |= flags[c];
            }
        }
        // min_by_key keeps the first of equals, i.e. the first found
        found.iter().min_by_key(|t| t.depth).map(|t| t.name)
    }
}

//! Versions of a GitHub pull request, as Gerrit's patch sets: its head
//! before each force-push and its current head, fetched into
//! refs/gh/pr/<n>/v<k>, with the base branch in refs/gh/pr/<n>/base. `git
//! range-diff` of each version against the one before chains the commits
//! that are revisions of the same patch into a change.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::process::Command;

use crate::git;

/// The versions of a pull request and the patches in them.
#[derive(Clone, Debug, Default)]
pub struct Pr {
    pub versions: usize,
    pub changes: Vec<Change>,
    /// Change and version of each commit id; the newest version for a
    /// commit in several.
    by_id: HashMap<String, (usize, usize)>,
}

/// One patch through the versions: its commit in each, if any.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub sets: Vec<Option<PatchSet>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PatchSet {
    pub id: String,
    /// The same patch as in the version before, maybe on another base.
    pub same: bool,
}

/// What happened to a change in a version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Absent,
    New,
    Changed,
    Same,
}

impl Change {
    pub fn marks(&self) -> Vec<Mark> {
        let mut prev = false;
        self.sets
            .iter()
            .map(|s| {
                let m = match s {
                    None => Mark::Absent,
                    Some(_) if !prev => Mark::New,
                    Some(p) if p.same => Mark::Same,
                    Some(_) => Mark::Changed,
                };
                prev = s.is_some();
                m
            })
            .collect()
    }

    /// The newest version before `v` that has this patch.
    pub fn before(&self, v: usize) -> Option<usize> {
        (0..v).rev().find(|&k| self.sets[k].is_some())
    }
}

impl Pr {
    /// The change of the commit `id` and the version it is in.
    pub fn find(&self, id: &str) -> Option<(&Change, usize)> {
        let &(c, v) = self.by_id.get(id)?;
        Some((&self.changes[c], v))
    }

    /// The pull request whose newest version `revs` (from `fetch`) show,
    /// None for other revisions or without a force-push.
    pub fn read(cwd: &PathBuf, revs: &[&str]) -> Option<Pr> {
        let (prefix, n) = revs.iter().find_map(|r| {
            let (prefix, v) = r.strip_prefix("refs/gh/pr/")?.rsplit_once("/v")?;
            Some((format!("refs/gh/pr/{prefix}"), v.parse::<usize>().ok()?))
        })?;
        if n < 2 {
            return None;
        }
        let base = format!("{prefix}/base");
        let mut pr = Pr { versions: n, ..Pr::default() };
        for k in 0..n - 1 {
            let range = |v: usize| format!("{base}..{prefix}/v{}", v + 1);
            let text =
                git(cwd, &["range-diff", "--no-color", "--abbrev=40", &range(k), &range(k + 1)])
                    .ok()?;
            for (old, new, same) in parse(&text) {
                let c = match old.as_ref().and_then(|o| pr.by_id.get(o)) {
                    Some(&(c, _)) => c,
                    None => {
                        pr.changes.push(Change { sets: vec![None; n] });
                        let c = pr.changes.len() - 1;
                        if let Some(o) = old {
                            pr.changes[c].sets[k] = Some(PatchSet { id: o.clone(), same: false });
                            pr.by_id.insert(o, (c, k));
                        }
                        c
                    }
                };
                if let Some(new) = new {
                    pr.changes[c].sets[k + 1] = Some(PatchSet { id: new.clone(), same });
                    pr.by_id.insert(new, (c, k + 1));
                }
            }
        }
        Some(pr)
    }
}

/// The range-diff of the patches of two commits, without its own header;
/// empty when they are the same.
pub fn interdiff(cwd: &PathBuf, old: &str, new: &str) -> io::Result<Vec<String>> {
    // pair the two however much they differ
    let (a, b) = (format!("{old}^!"), format!("{new}^!"));
    let text = git(cwd, &["range-diff", "--no-color", "--creation-factor=999", &a, &b])?;
    Ok(text.lines().filter_map(|l| l.strip_prefix("    ")).map(String::from).collect())
}

/// Fetches the versions of pull request `n` of the GitHub repository `gh`
/// picks for `cwd`. Returns the `git log` arguments that show the newest.
pub fn fetch(cwd: &PathBuf, n: u32) -> io::Result<Vec<String>> {
    let query = "query($owner:String!,$name:String!,$n:Int!){repository(owner:$owner,name:$name){
        url pullRequest(number:$n){baseRefName headRefOid
        timelineItems(itemTypes:[HEAD_REF_FORCE_PUSHED_EVENT],first:100){
        nodes{... on HeadRefForcePushedEvent{beforeCommit{oid}}}}}}}";
    let out = Command::new("gh")
        .current_dir(cwd)
        .args(["api", "graphql", "-F", "owner={owner}", "-F", "name={repo}"])
        .args(["-F", &format!("n={n}"), "-f", &format!("query={query}")])
        .args(["-q", ".data.repository | .url, .pullRequest.baseRefName, (.pullRequest.timelineItems.nodes[].beforeCommit.oid // empty), .pullRequest.headRefOid"])
        .output()
        .map_err(|e| io::Error::other(format!("gh: {e}")))?;
    if !out.status.success() {
        return Err(io::Error::other(String::from_utf8_lossy(&out.stderr).trim().to_string()));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines();
    let (Some(url), Some(base)) = (lines.next(), lines.next()) else {
        return Err(io::Error::other(format!("no pull request {n}")));
    };
    let mut heads: Vec<&str> = lines.collect();
    heads.dedup();
    let mut args = vec!["fetch".to_string(), "--no-tags".into(), "-q".into(), url.into()];
    args.push(format!("+refs/heads/{base}:refs/gh/pr/{n}/base"));
    for (k, h) in heads.iter().enumerate() {
        args.push(format!("+{h}:refs/gh/pr/{n}/v{}", k + 1));
    }
    git(cwd, &args.iter().map(String::as_str).collect::<Vec<_>>())?;
    // the newest version; the older ones are patch sets of its commits
    Ok(vec![format!("refs/gh/pr/{n}/v{}", heads.len()), format!("^refs/gh/pr/{n}/base")])
}

/// `git range-diff` output as (old commit, new commit, same patch).
fn parse(text: &str) -> Vec<(Option<String>, Option<String>, bool)> {
    let id = |s: &str| (!s.starts_with('-')).then(|| s.to_string());
    text.lines()
        // the patch lines are indented by four, the pair lines by less
        .filter(|l| !l.starts_with("    "))
        .filter_map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            let [_, old, mark, _, new, ..] = w[..] else { return None };
            Some((id(old), id(new), mark == "="))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_sets_of_each_change() {
        let (dir, sh) = crate::tests::scratch("pr");
        let commit = |file: &str, text: &str, msg: &str| {
            std::fs::write(dir.join(file), text).unwrap();
            sh(&["add", file]);
            sh(&["commit", "-q", "-m", msg]);
        };
        commit("base", "x\n", "base");
        sh(&["update-ref", "refs/gh/pr/1/base", "HEAD"]);
        commit("a", "1\n", "a");
        // range-diff pairs patches that mostly agree
        let lines: String = (1..10).map(|i| format!("line {i}\n")).collect();
        commit("b", &format!("{lines}1\n"), "b");
        sh(&["update-ref", "refs/gh/pr/1/v1", "HEAD"]);
        sh(&["reset", "-q", "--hard", "HEAD^"]);
        commit("b", &format!("{lines}2\n"), "b");
        commit("c", "1\n", "c");
        sh(&["update-ref", "refs/gh/pr/1/v2", "HEAD"]);
        let args: Vec<String> = ["refs/gh/pr/1/v2", "^refs/gh/pr/1/base"].map(String::from).into();
        let repo = crate::Repo::open(&dir, &args).unwrap();
        // the newest version only, a patch per row
        let subjects: Vec<&str> = repo.commits.iter().map(|c| c.subject()).collect();
        assert_eq!(subjects, ["c", "b", "a"]);
        let marks =
            |row: usize| repo.pr.as_ref().unwrap().find(repo.commits[row].id()).unwrap().0.marks();
        assert_eq!(marks(0), [Mark::Absent, Mark::New]);
        assert_eq!(marks(1), [Mark::New, Mark::Changed]);
        assert_eq!(marks(2), [Mark::New, Mark::Same]);
        let text = |set: Option<usize>| {
            let d = repo.patch_set_details(1, set, None, &crate::DiffOpts::default());
            d.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")
        };
        let b = text(None);
        assert!(b.contains("Patch set v2 of 2, changed since v1"), "{b}");
        assert!(b.contains("Changes since v1") && b.contains("-+1") && b.contains("++2"), "{b}");
        // the first patch set, with its own diff
        let b1 = text(Some(0));
        assert!(b1.contains("Patch set v1 of 2, new") && b1.contains("\n+1"), "{b1}");
        assert!(!b1.contains("Changes since"), "{b1}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn parses_range_diff() {
        let text = " 1:  aaaa =  1:  aaaa same
 2:  bbbb =  2:  cccc rebased
 3:  dddd !  3:  eeee changed
    @@ a.go
    -x
    +y
 4:  ffff <  -:  ---- dropped
 -:  ---- >  4:  1111 added
";
        let id = |s: &str| Some(s.to_string());
        assert_eq!(
            parse(text),
            vec![
                (id("aaaa"), id("aaaa"), true),
                (id("bbbb"), id("cccc"), true),
                (id("dddd"), id("eeee"), false),
                (id("ffff"), None, false),
                (None, id("1111"), false),
            ]
        );
    }
}

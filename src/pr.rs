//! Versions of a GitHub pull request: its head before each force-push and
//! its current head, fetched into refs/pr/<n>/v<k>, with the base branch
//! in refs/pr/<n>/base. Each commit of a version knows the commit it
//! replaced in the version before, from `git range-diff`.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::process::Command;

use crate::git;

/// How a commit of version `version` relates to the previous version.
#[derive(Clone, Debug, PartialEq)]
pub struct Revision {
    pub version: usize,
    /// The commit it replaced; None for a new commit.
    pub old: Option<String>,
    /// The range-diff of the two patches, empty when they are the same.
    pub diff: Vec<String>,
}

/// Fetches the versions of pull request `n` of the GitHub repository `gh`
/// picks for `cwd`. Returns the `git log` arguments that show them.
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
    args.push(format!("+refs/heads/{base}:refs/pr/{n}/base"));
    let mut revs = vec![];
    for (k, h) in heads.iter().enumerate() {
        args.push(format!("+{h}:refs/pr/{n}/v{}", k + 1));
        revs.push(format!("refs/pr/{n}/v{}", k + 1));
    }
    git(cwd, &args.iter().map(String::as_str).collect::<Vec<_>>())?;
    revs.push(format!("^refs/pr/{n}/base"));
    Ok(revs)
}

/// The revision of each commit, by id, when `revs` are the arguments
/// `fetch` returned; empty otherwise. A commit already in the previous
/// version has none.
pub fn revisions(cwd: &PathBuf, revs: &[&str]) -> HashMap<String, Revision> {
    let mut out = HashMap::new();
    let Some(base) =
        revs.iter().find_map(|r| r.strip_prefix('^')).filter(|b| b.starts_with("refs/pr/"))
    else {
        return out;
    };
    let versions: Vec<&str> = revs.iter().copied().filter(|r| r.starts_with("refs/pr/")).collect();
    for (k, pair) in versions.windows(2).enumerate() {
        let (old, new) = (format!("{base}..{}", pair[0]), format!("{base}..{}", pair[1]));
        let Ok(text) = git(cwd, &["range-diff", "--no-color", "--abbrev=40", &old, &new]) else {
            continue;
        };
        for (id, rev) in parse(&text, k + 2) {
            out.entry(id).or_insert(rev);
        }
    }
    out
}

/// `git range-diff` output as (new commit, revision) pairs.
fn parse(text: &str, version: usize) -> Vec<(String, Revision)> {
    let mut out: Vec<(String, Revision)> = vec![];
    for line in text.lines() {
        // the patch lines are indented by four, the pair lines by less
        if let Some(l) = line.strip_prefix("    ") {
            if let Some((_, rev)) = out.last_mut() {
                rev.diff.push(l.to_string());
            }
            continue;
        }
        let w: Vec<&str> = line.split_whitespace().collect();
        let [_, old, mark, _, new, ..] = w[..] else { continue };
        if new.starts_with('-') || (mark == "=" && old == new) {
            // dropped, or the same commit; push a stand-in for the patch lines
            out.push((String::new(), Revision { version, old: None, diff: vec![] }));
            continue;
        }
        let old = (!old.starts_with('-')).then(|| old.to_string());
        out.push((new.to_string(), Revision { version, old, diff: vec![] }));
    }
    out.retain(|(id, _)| !id.is_empty());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn details_show_the_previous_version() {
        let (dir, sh) = crate::tests::scratch("pr");
        let commit = |file: &str, text: &str, msg: &str| {
            std::fs::write(dir.join(file), text).unwrap();
            sh(&["add", file]);
            sh(&["commit", "-q", "-m", msg]);
        };
        commit("base", "x\n", "base");
        sh(&["update-ref", "refs/pr/1/base", "HEAD"]);
        commit("a", "1\n", "a");
        // range-diff pairs patches that mostly agree
        let lines: String = (1..10).map(|i| format!("line {i}\n")).collect();
        commit("b", &format!("{lines}1\n"), "b");
        sh(&["update-ref", "refs/pr/1/v1", "HEAD"]);
        sh(&["reset", "-q", "--hard", "HEAD^"]);
        commit("b", &format!("{lines}2\n"), "b");
        commit("c", "1\n", "c");
        sh(&["update-ref", "refs/pr/1/v2", "HEAD"]);
        let args: Vec<String> =
            ["refs/pr/1/v1", "refs/pr/1/v2", "^refs/pr/1/base"].map(String::from).into();
        let repo = crate::Repo::open(&dir, &args).unwrap();
        let text = |subject: &str, version: usize| {
            let row = repo
                .commits
                .iter()
                .position(|c| {
                    c.subject() == subject
                        && repo.revisions.get(c.id()).map_or(1, |r| r.version) == version
                })
                .unwrap();
            let d = repo.details(row, &crate::DiffOpts::default());
            d.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")
        };
        assert!(!text("a", 1).contains("v1"));
        let b = text("b", 2);
        assert!(b.contains("Changed from v1") && b.contains("Changes since v1"), "{b}");
        assert!(b.contains("-+1") && b.contains("++2"), "{b}");
        assert!(text("c", 2).contains("New in v2"));
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
        let got = parse(text, 2);
        let rev = |old: Option<&str>, diff: &[&str]| Revision {
            version: 2,
            old: old.map(String::from),
            diff: diff.iter().map(|s| s.to_string()).collect(),
        };
        assert_eq!(
            got,
            vec![
                ("cccc".to_string(), rev(Some("bbbb"), &[])),
                ("eeee".to_string(), rev(Some("dddd"), &["@@ a.go", "-x", "+y"])),
                ("1111".to_string(), rev(None, &[])),
            ]
        );
    }
}

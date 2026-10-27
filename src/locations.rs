//! Branches, remotes, tags, stashes, submodules and worktrees for the sidebar.

use std::path::{Path, PathBuf};

use crate::git;

#[derive(Clone, Debug, Default)]
pub struct Location {
    /// Shown name; branches, remotes and tags nest on '/'.
    pub name: String,
    /// Commit id; empty when unknown.
    pub id: String,
    /// Checked out branch or worktree.
    pub current: bool,
    /// Commits (behind, ahead) of the upstream branch.
    pub track: Option<(usize, usize)>,
    /// Branches: ids of the commits not on the default branch.
    pub own: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Locations {
    pub branches: Vec<Location>,
    pub remotes: Vec<Location>,
    pub tags: Vec<Location>,
    pub stashes: Vec<Location>,
    pub submodules: Vec<Location>,
    pub worktrees: Vec<Location>,
}

impl Locations {
    pub fn read(cwd: &PathBuf) -> Locations {
        let mut l = Locations::default();
        let refs = git(cwd, &[
            "for-each-ref",
            "--format=%(refname)%00%(objectname)%00%(*objectname)%00%(HEAD)%00%(upstream:track,nobracket)",
            "refs/heads", "refs/remotes", "refs/tags",
        ]);
        for line in refs.unwrap_or_default().lines() {
            let f: Vec<&str> = line.split('\0').collect();
            let [r, id, peeled, head, track] = f[..] else { continue };
            // a tag's peeled id is the commit; plain refs have none
            let id = if peeled.is_empty() { id } else { peeled };
            let loc = |name: &str| Location {
                name: name.into(),
                id: id.into(),
                current: head == "*",
                track: parse_track(track),
                ..Default::default()
            };
            if let Some(b) = r.strip_prefix("refs/heads/") {
                l.branches.push(loc(b));
            } else if let Some(b) = r.strip_prefix("refs/remotes/") {
                if !b.ends_with("/HEAD") {
                    l.remotes.push(loc(b));
                }
            } else if let Some(t) = r.strip_prefix("refs/tags/") {
                l.tags.push(loc(t));
            }
        }

        let stashes = git(cwd, &["stash", "list", "--format=%H%x00%gd: %s"]);
        for line in stashes.unwrap_or_default().lines() {
            if let Some((id, name)) = line.split_once('\0') {
                l.stashes.push(Location { name: name.into(), id: id.into(), ..Default::default() });
            }
        }

        // " <id> <path> (<describe>)", the first column is a status flag
        let subs = git(cwd, &["submodule", "status"]);
        for line in subs.unwrap_or_default().lines() {
            let mut f = line.get(1..).unwrap_or("").split(' ');
            if let (Some(id), Some(path)) = (f.next(), f.next()) {
                l.submodules.push(Location {
                    name: path.into(),
                    id: id.into(),
                    ..Default::default()
                });
            }
        }

        let top = git(cwd, &["rev-parse", "--show-toplevel"]).unwrap_or_default();
        let wts = git(cwd, &["worktree", "list", "--porcelain"]);
        for block in wts.unwrap_or_default().split("\n\n") {
            let mut w = Location::default();
            let mut branch = None;
            for line in block.lines() {
                if let Some(p) = line.strip_prefix("worktree ") {
                    w.name =
                        Path::new(p).file_name().map_or(p.into(), |n| n.to_string_lossy().into());
                    w.current = p == top.trim();
                } else if let Some(id) = line.strip_prefix("HEAD ") {
                    w.id = id.into();
                } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
                    branch = Some(b.to_string());
                }
            }
            if !w.name.is_empty() {
                if let Some(b) = branch {
                    w.name = format!("{} ({b})", w.name);
                }
                l.worktrees.push(w);
            }
        }
        l
    }
}

/// "ahead 2, behind 1004" -> (1004, 2); None without an upstream or when gone.
fn parse_track(s: &str) -> Option<(usize, usize)> {
    if s.is_empty() || s == "gone" {
        return None;
    }
    let (mut behind, mut ahead) = (0, 0);
    for part in s.split(", ") {
        match part.split_once(' ') {
            Some(("ahead", n)) => ahead = n.parse().ok()?,
            Some(("behind", n)) => behind = n.parse().ok()?,
            _ => {}
        }
    }
    Some((behind, ahead))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track() {
        assert_eq!(parse_track(""), None);
        assert_eq!(parse_track("gone"), None);
        assert_eq!(parse_track("up to date"), Some((0, 0)));
        assert_eq!(parse_track("ahead 2"), Some((0, 2)));
        assert_eq!(parse_track("ahead 2, behind 1004"), Some((1004, 2)));
    }

    #[test]
    fn read_this_repo() {
        let l = Locations::read(&PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        assert!(l.branches.iter().any(|b| b.current && !b.id.is_empty()));
        assert!(l.worktrees.iter().any(|w| w.current));
    }
}

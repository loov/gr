//! Core of the viewer: loading history, graph layout,
//! commit details/diffs, find, goto and navigation history.

mod annotations;
mod details;
mod find;
mod graph;
mod locations;
mod reach;

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub use annotations::Annotations;
pub use details::{DiffOpts, Details, Kind, Line, Style};
pub use find::{Field, Hit, Mode, Query};
pub use graph::Seg;
pub use locations::{Location, Locations};

pub type Rgb = [u8; 3];

/// Fake commit id for "Local uncommitted changes, not checked in to index".
pub const UNCOMMITTED_ID: &str = "0000000000000000000000000000000000000000";
/// Fake commit id for "Local changes checked in to index but not committed".
pub const INDEX_ID: &str = "0000000000000000000000000000000000000001";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    Tag,
    Head,
    Remote,
    Other,
}

#[derive(Clone, Debug)]
pub struct Ref {
    pub name: String,
    pub kind: RefKind,
    /// The currently checked out branch; drawn bold.
    pub current: bool,
}

impl Ref {
    /// Label (background, foreground) colours, gitk defaults.
    pub fn colors(&self) -> (Rgb, Rgb) {
        match self.kind {
            RefKind::Tag => ([255, 255, 0], [0, 0, 0]),
            RefKind::Head | RefKind::Remote => ([0, 255, 0], [0, 0, 0]),
            RefKind::Other => ([0xdd, 0xdd, 0xff], [0, 0, 0]),
        }
    }

    /// Length of the `remotes/<name>/` prefix, which gitk shades `#ffddaa`.
    pub fn remote_prefix_len(&self) -> usize {
        if self.kind != RefKind::Remote {
            return 0;
        }
        match self.name["remotes/".len()..].find('/') {
            Some(i) => "remotes/".len() + i + 1,
            None => 0,
        }
    }
}

pub const REMOTE_PREFIX_BG: Rgb = [0xff, 0xdd, 0xaa];

#[derive(Clone, Debug)]
pub struct Commit {
    /// id, parents, author, committer and message, in one allocation;
    /// `ends` marks where each but the last ends.
    text: Box<str>,
    ends: [u32; 4],
    /// Unix times, formatted only when shown.
    author_time: i64,
    committer_time: i64,
    /// Row indices of children in this view.
    pub children: Vec<usize>,
    pub refs: Vec<Ref>,
    /// Graph: lane of the node, in lane units.
    pub lane: usize,
    pub node_fill: Rgb,
    /// Number of lanes used by this row; text starts after this.
    pub width: usize,
}

impl Commit {
    /// `author` and `committer` are "Name <email>".
    pub(crate) fn new(id: &str, parents: &str, author: &str, author_time: i64, committer: &str, committer_time: i64, body: &str) -> Commit {
        let fields = [id, parents, author, committer, body];
        let mut ends = [0; 4];
        let mut end = 0;
        for (e, f) in ends.iter_mut().zip(fields) {
            end += f.len();
            *e = end as u32;
            end += 1;
        }
        Commit {
            text: fields.join("\x1f").into(),
            ends,
            author_time,
            committer_time,
            children: vec![],
            refs: vec![],
            lane: 0,
            node_fill: [0; 3],
            width: 0,
        }
    }

    fn field(&self, i: usize) -> &str {
        let start = if i == 0 { 0 } else { self.ends[i - 1] as usize + 1 };
        let end = self.ends.get(i).map_or(self.text.len(), |&e| e as usize);
        &self.text[start..end]
    }

    pub fn id(&self) -> &str {
        self.field(0)
    }
    pub fn parents(&self) -> impl Iterator<Item = &str> {
        self.field(1).split_whitespace()
    }
    /// "Name <email>"
    pub fn author(&self) -> &str {
        self.field(2)
    }
    /// git strips '<' from names, so the name ends at the first " <".
    pub fn author_name(&self) -> &str {
        let a = self.author();
        a.find(" <").map_or(a, |i| &a[..i])
    }
    /// "YYYY-MM-DD HH:MM:SS" in local time; empty for the fake commits.
    pub fn author_date(&self) -> String {
        if self.is_fake() { String::new() } else { local_time(self.author_time) }
    }
    pub fn committer(&self) -> &str {
        self.field(3)
    }
    pub fn committer_date(&self) -> String {
        if self.is_fake() { String::new() } else { local_time(self.committer_time) }
    }
    pub fn committer_name(&self) -> &str {
        let c = self.committer();
        c.find(" <").map_or(c, |i| &c[..i])
    }
    /// Commit time relative to `now`: "14:05" today, "Oct 21" this year,
    /// else "2024-03-05". Empty for the fake commits.
    pub fn committer_date_short(&self, now: i64) -> String {
        if self.is_fake() {
            return String::new();
        }
        short_time(&local_time(self.committer_time), &local_time(now))
    }
    pub fn body(&self) -> &str {
        self.field(4)
    }
    /// First non-blank line of the message.
    pub fn subject(&self) -> &str {
        self.body().lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("")
    }

    pub fn is_fake(&self) -> bool {
        self.id() == UNCOMMITTED_ID || self.id() == INDEX_ID
    }
}

pub struct Repo {
    /// Top level of the work tree (or the git dir); names the window.
    pub dir: PathBuf,
    /// Directory git runs in, so relative path limits keep their meaning.
    pub(crate) cwd: PathBuf,
    log_args: Vec<String>,
    pub commits: Vec<Commit>,
    /// Row of HEAD, the initial selection.
    pub head_row: Option<usize>,
    /// Path limit from the command line; diffs are limited to it too.
    pub(crate) paths: Vec<String>,
    /// (first 16 hex digits of the id, row), sorted, for `index_of`.
    by_id: Vec<(u64, u32)>,
    /// Graph lane state every few rows, for `segs`.
    snapshots: Vec<graph::Lanes>,
    tags: HashMap<String, String>,
    heads: HashMap<String, String>,
    /// near_refs results per commit id.
    pub(crate) near: RefCell<HashMap<String, Vec<(&'static str, String)>>>,
    /// The whole commit graph, loaded on a thread started by `open`.
    reach_job: RefCell<Option<std::thread::JoinHandle<Option<reach::Reach>>>>,
    reach: OnceCell<Option<reach::Reach>>,
}

pub(crate) fn git(dir: &PathBuf, args: &[&str]) -> io::Result<String> {
    let out = Command::new("git")
        .current_dir(dir)
        .args(["-c", "i18n.logOutputEncoding=UTF-8"])
        .args(args)
        .output()?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(io::Error::other(msg));
    }
    // no copy when the output is valid UTF-8, which it nearly always is
    Ok(String::from_utf8(out.stdout).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))
}

/// Streams `git log -z` output, parsing records as they arrive.
fn git_log(dir: &PathBuf, args: &[&str]) -> io::Result<Vec<Commit>> {
    let mut child = Command::new("git")
        .current_dir(dir)
        .args(["-c", "i18n.logOutputEncoding=UTF-8"])
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut out = BufReader::with_capacity(1 << 16, child.stdout.take().unwrap());
    let mut commits = vec![];
    let mut rec = vec![];
    while out.read_until(0, &mut rec)? > 0 {
        if rec.last() == Some(&0) {
            rec.pop();
        }
        let r = String::from_utf8_lossy(&rec);
        if !r.trim().is_empty() {
            commits.extend(parse_record(&r));
        }
        rec.clear();
    }
    let mut err = String::new();
    child.stderr.take().unwrap().read_to_string(&mut err)?;
    if !child.wait()?.success() {
        return Err(io::Error::other(err.trim().to_string()));
    }
    Ok(commits)
}

/// "YYYY-MM-DD HH:MM:SS" in local time, like git's --date=format-local.
fn local_time(t: i64) -> String {
    let tt = t as libc::time_t;
    // SAFETY: both pointers are valid for the call; tm is plain data.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    #[cfg(unix)]
    let ok = unsafe { !libc::localtime_r(&tt, &mut tm).is_null() };
    #[cfg(windows)]
    let ok = unsafe { libc::localtime_s(&mut tm, &tt) == 0 };
    if !ok {
        return String::new();
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

fn short_time(t: &str, now: &str) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    if t.len() < 16 || now.len() < 10 {
        return t.to_string();
    }
    if t[..10] == now[..10] {
        return t[11..16].to_string();
    }
    if t[..4] == now[..4] {
        let m: usize = t[5..7].parse().unwrap_or(1);
        let d: u32 = t[8..10].parse().unwrap_or(1);
        return format!("{} {d}", MONTHS[m.clamp(1, 12) - 1]);
    }
    t[..10].to_string()
}

/// Runs a `--quiet` style git command; true when it reports differences (exit 1).
fn git_differs(dir: &PathBuf, args: &[&str]) -> bool {
    Command::new("git")
        .current_dir(dir)
        .args(args)
        .stderr(Stdio::null())
        .status()
        .map(|s| s.code() == Some(1))
        .unwrap_or(false)
}

impl Repo {
    /// Loads the whole history synchronously; `log_args` are passed to
    /// `git log` (default `--all`, so every branch in the sidebar is listed).
    // Synchronous load; stream in the background if huge repos (>100k commits) matter.
    pub fn open(dir: impl Into<PathBuf>, log_args: &[String]) -> io::Result<Repo> {
        let cwd: PathBuf = dir.into();
        let cwd = cwd.canonicalize().unwrap_or(cwd);
        let toplevel = git(&cwd, &["rev-parse", "--show-toplevel"]).ok();
        let dir = toplevel.map(|t| PathBuf::from(t.trim())).unwrap_or_else(|| cwd.clone());

        let mut args = vec![
            "log",
            "--topo-order",
            // --parents makes %P show rewritten parents under path limiting
            "--parents",
            "-z",
            "--encoding=UTF-8",
            // raw times: formatting them here is cheaper than git's format-local
            "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%cn%x1f%ce%x1f%ct%x1f%B",
        ];
        let (revs, paths) = match log_args.iter().position(|a| a == "--") {
            Some(i) => (&log_args[..i], log_args[i + 1..].to_vec()),
            None => (log_args, vec![]),
        };
        let revs: Vec<&str> = revs.iter().map(String::as_str).filter(|a| !breaks_parsing(a)).collect();
        let default_revs = revs.iter().all(|a| a.starts_with('-'));
        if default_revs {
            args.push("--all");
        }
        args.extend(revs);
        args.push("--");
        args.extend(paths.iter().map(String::as_str));
        let reach_job = {
            let cwd = cwd.clone();
            std::thread::spawn(move || reach::Reach::read(&cwd))
        };
        // the other git calls are quick; run them while git log walks the history
        let (commits, side) = std::thread::scope(|s| {
            let side = s.spawn(|| Side::read(&cwd, &paths));
            (git_log(&cwd, &args), side.join().unwrap())
        });
        let mut commits = commits?;
        if commits.is_empty() && default_revs && side.head.is_empty() {
            return Err(io::Error::other("no commits yet"));
        }
        if commits.is_empty() {
            return Err(io::Error::other("No commits selected"));
        }

        let Side { mut head, head_branch, view_head, staged, unstaged, refs } = side;
        let mut fake = vec![];
        if commits.iter().any(|c| c.id() == view_head) {
            if unstaged {
                let parent = if staged { INDEX_ID } else { &view_head };
                fake.push(fake_commit(UNCOMMITTED_ID, parent, "Local uncommitted changes, not checked in to index", [255, 0, 0]));
            }
            if staged {
                fake.push(fake_commit(INDEX_ID, &view_head, "Local changes checked in to index but not committed", [0, 255, 0]));
            }
        }
        if !commits.iter().any(|c| c.id() == head) {
            head = view_head;
        }
        commits.splice(0..0, fake);

        let mut repo = Repo {
            dir,
            cwd,
            log_args: log_args.to_vec(),
            near: RefCell::default(),
            reach_job: RefCell::new(Some(reach_job)),
            reach: OnceCell::new(),
            paths,
            head_row: None,
            by_id: vec![],
            snapshots: vec![],
            tags: HashMap::new(),
            heads: HashMap::new(),
            commits,
        };
        repo.by_id = repo.commits.iter().enumerate().map(|(i, c)| (id_key(c.id()).unwrap_or(0), i as u32)).collect();
        repo.by_id.sort_unstable();
        let edges: Vec<(usize, usize)> = (0..repo.commits.len())
            .flat_map(|i| repo.commits[i].parents().filter_map(|p| repo.index_of(p)).map(move |pi| (pi, i)).collect::<Vec<_>>())
            .collect();
        for (pi, i) in edges {
            repo.commits[pi].children.push(i);
        }
        repo.head_row = repo.index_of(&head);
        repo.read_refs(&refs, head_branch.as_deref());
        for c in &mut repo.commits {
            if !c.is_fake() {
                c.node_fill = if c.id() == head { [255, 255, 0] } else { [0, 0, 255] };
            }
        }
        let (placed, snapshots) = graph::layout(&repo.commits, |id| repo.index_of(id));
        for (c, (lane, width)) in repo.commits.iter_mut().zip(placed) {
            (c.lane, c.width) = (lane, width);
        }
        repo.snapshots = snapshots;
        Ok(repo)
    }

    /// The whole commit graph, waiting for its loader thread on first use.
    pub(crate) fn reach(&self) -> Option<&reach::Reach> {
        self.reach.get_or_init(|| self.reach_job.borrow_mut().take().and_then(|h| h.join().ok()).flatten()).as_ref()
    }

    /// Graph line segments of `row`, in lane units (x) and row units (y, 0..1).
    pub fn segs(&self, row: usize) -> Vec<Seg> {
        graph::segs(&self.commits, &self.snapshots, row, |id| self.index_of(id))
    }

    /// Reopens with the same directory and log arguments. Returns the new
    /// repo and the row to select: `sel`'s commit if still listed, else the
    /// first commit with its subject (it was amended or rebased), else the
    /// commit as far down the same branch, else HEAD's row or 0.
    pub fn reload(&self, sel: usize) -> io::Result<(Repo, usize)> {
        let repo = Repo::open(&self.cwd, &self.log_args)?;
        let old = self.commits.get(sel);
        let row = old
            .and_then(|c| repo.index_of(c.id()))
            .or_else(|| old.and_then(|c| repo.commits.iter().position(|n| !n.is_fake() && n.subject() == c.subject())))
            .or_else(|| {
                let (branch, depth) = self.branch_pos(sel)?;
                let mut row = repo.index_of(repo.heads.get(&branch)?)?;
                for _ in 0..depth {
                    match repo.first_parent(row) {
                        Some(p) => row = p,
                        None => break,
                    }
                }
                Some(row)
            });
        let row = row.or(repo.head_row).unwrap_or(0);
        Ok((repo, row))
    }

    fn first_parent(&self, row: usize) -> Option<usize> {
        self.index_of(self.commits[row].parents().next()?)
    }

    /// A branch with `row` on its first-parent chain and how many commits
    /// below the tip it is. The checked out branch goes first, then local
    /// ones, so the answer names the branch being worked on.
    fn branch_pos(&self, row: usize) -> Option<(String, usize)> {
        let cur = self.commits.iter().flat_map(|c| &c.refs).find(|r| r.current).map(|r| r.name.as_str());
        let mut names: Vec<&String> = self.heads.keys().collect();
        names.sort_by_key(|n| (Some(n.as_str()) != cur, n.starts_with("remotes/"), n.as_str()));
        names.into_iter().find_map(|n| {
            let mut i = self.index_of(&self.heads[n])?;
            let mut depth = 0;
            while i != row {
                i = self.first_parent(i)?;
                depth += 1;
            }
            Some((n.clone(), depth))
        })
    }

    fn read_refs(&mut self, out: &str, head_branch: Option<&str>) {
        let mut tags = vec![];
        let mut others = vec![];
        for line in out.lines() {
            let Some((id, r)) = line.split_once(' ') else { continue };
            let Some(name) = r.strip_prefix("refs/") else { continue };
            if let Some(b) = name.strip_prefix("heads/") {
                self.heads.insert(b.to_string(), id.to_string());
                others.push((id, Ref { name: b.to_string(), kind: RefKind::Head, current: Some(b) == head_branch }));
            } else if name.starts_with("remotes/") {
                if !name.ends_with("/HEAD") {
                    self.heads.insert(name.to_string(), id.to_string());
                    others.push((id, Ref { name: name.to_string(), kind: RefKind::Remote, current: false }));
                }
            } else if let Some(t) = name.strip_prefix("tags/") {
                // refs/tags/x^{} (the peeled commit) overrides the tag object id
                let t = t.strip_suffix("^{}").unwrap_or(t);
                self.tags.insert(t.to_string(), id.to_string());
            } else {
                others.push((id, Ref { name: name.to_string(), kind: RefKind::Other, current: false }));
            }
        }
        for (t, id) in &self.tags {
            tags.push((id.clone(), Ref { name: t.clone(), kind: RefKind::Tag, current: false }));
        }
        tags.sort_by(|a, b| a.1.name.cmp(&b.1.name));
        for (id, r) in tags.iter().map(|(id, r)| (id.as_str(), r.clone())).chain(others) {
            if let Some(i) = self.index_of(id) {
                self.commits[i].refs.push(r);
            }
        }
        // tags first, then heads (local before remote), then other refs
        for c in &mut self.commits {
            c.refs.sort_by_key(|r| match r.kind {
                RefKind::Tag => 0,
                RefKind::Head => 1,
                RefKind::Remote => 2,
                RefKind::Other => 3,
            });
        }
    }

    /// Annotations from `gr.info` in the git dir, shared by all worktrees.
    pub fn annotations(&self) -> io::Result<Annotations> {
        let dir = git(&self.cwd, &["rev-parse", "--git-common-dir"])?;
        Annotations::load(self.cwd.join(dir.trim()).join("gr.info"))
    }

    /// Branches, tags and the like, for the sidebar.
    pub fn locations(&self) -> Locations {
        let mut l = Locations::read(&self.cwd);
        // ponytail: main/master stand in for the default branch; read origin/HEAD if other names matter
        let base = ["main", "master", "remotes/origin/main", "remotes/origin/master"];
        let mut on_base = vec![false; self.commits.len()];
        self.walk(base.iter().filter_map(|b| self.heads.get(*b)), &mut on_base, |_| {});
        for b in &mut l.branches {
            let mut seen = on_base.clone();
            self.walk([&b.id], &mut seen, |c| b.own.push(c.id().to_string()));
        }
        l
    }

    /// Calls `f` on every commit reachable from `tips` and not yet `seen`, marking it seen.
    fn walk<'a>(&self, tips: impl IntoIterator<Item = &'a String>, seen: &mut [bool], mut f: impl FnMut(&Commit)) {
        let mut stack: Vec<usize> = tips.into_iter().filter_map(|id| self.index_of(id)).collect();
        while let Some(i) = stack.pop() {
            if std::mem::replace(&mut seen[i], true) {
                continue;
            }
            let c = &self.commits[i];
            f(c);
            stack.extend(c.parents().filter_map(|p| self.index_of(p)));
        }
    }

    /// Window title, "<repo>: All files - gitk".
    pub fn title(&self) -> String {
        let name = self.dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        format!("{name}: All files - gitk")
    }

    pub fn index_of(&self, id: &str) -> Option<usize> {
        let key = id_key(id)?;
        let from = self.by_id.partition_point(|&(k, _)| k < key);
        self.by_id[from..]
            .iter()
            .take_while(|&&(k, _)| k == key)
            .map(|&(_, i)| i as usize)
            .find(|&i| self.commits[i].id() == id)
    }

    /// Resolves the Goto field: tag, branch, unique id prefix, or any rev expression.
    pub fn resolve(&self, s: &str) -> Result<usize, String> {
        let s = s.trim();
        if s.is_empty() {
            return Err("Nothing to go to".into());
        }
        if let Some(i) = self.index_of(s) {
            return Ok(i);
        }
        let id = if let Some(id) = self.tags.get(s).or_else(|| self.heads.get(s)) {
            id.clone()
        } else if s.len() >= 4 && s.len() < 64 && s.chars().all(|c| c.is_ascii_hexdigit()) {
            let p = s.to_ascii_lowercase();
            let mut m = self.commits.iter().filter(|c| !c.is_fake() && c.id().starts_with(&p));
            match (m.next(), m.next()) {
                (Some(c), None) => c.id().to_string(),
                (Some(_), Some(_)) => return Err(format!("Short commit ID {s} is ambiguous")),
                _ => return Err(format!("Commit ID {s} is not known")),
            }
        } else {
            match git(&self.cwd, &["rev-parse", "--verify", "-q", &format!("{s}^{{commit}}")]) {
                Ok(id) => id.trim().to_string(),
                Err(_) => return Err(format!("Revision {s} is not known")),
            }
        };
        self.index_of(&id)
            .ok_or_else(|| format!("Revision {s} is not in the current view"))
    }
}

/// git log options that would change the output format or order we rely on,
/// or add diff output to the records.
fn breaks_parsing(arg: &str) -> bool {
    const EXACT: &[&str] = &[
        "--oneline", "--reverse", "-z", "--graph", "--name-only", "--name-status", "-p", "-u",
        "--patch", "--raw", "--numstat", "--shortstat", "--summary", "--decorate", "--abbrev-commit",
        "-g", "--walk-reflogs", "--no-walk", "--parents", "--children", "--boundary", "--header",
        "--log-size", "--patch-with-stat", "--patch-with-raw", "-c", "--cc", "-m",
        "--combined-all-paths", "--remerge-diff", "--compact-summary", "--binary", "--check",
        "-W", "--function-context", "--show-signature", "--left-right", "--relative-date",
    ];
    const PREFIX: &[&str] = &[
        "--pretty", "--format", "--date=", "--color", "--stat", "--decorate=", "-U", "--unified",
        "--dirstat", "--word-diff", "--diff-merges", "--encoding",
    ];
    EXACT.contains(&arg) || PREFIX.iter().any(|p| arg.starts_with(p))
}

fn is_id(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn fake_commit(id: &str, parent: &str, subject: &str, fill: Rgb) -> Commit {
    let mut c = Commit::new(id, parent, "", 0, "", 0, subject);
    c.node_fill = fill;
    c
}

fn parse_record(r: &str) -> Option<Commit> {
    let f: Vec<&str> = r.trim_start_matches('\n').splitn(9, '\x1f').collect();
    if f.len() < 9 || !is_id(f[0]) || !f[1].split_whitespace().all(is_id) {
        return None;
    }
    let author = format!("{} <{}>", f[2], f[3]);
    let committer = format!("{} <{}>", f[5], f[6]);
    Some(Commit::new(f[0], f[1], &author, f[4].parse().ok()?, &committer, f[7].parse().ok()?, f[8].trim_end()))
}

/// Sort key for `index_of`: the id's first 16 hex digits.
fn id_key(id: &str) -> Option<u64> {
    u64::from_str_radix(id.get(..16)?, 16).ok()
}

/// What `open` needs from git besides the log; gathered on another thread.
struct Side {
    head: String,
    head_branch: Option<String>,
    /// HEAD, or with a path limit the newest commit touching the paths.
    view_head: String,
    staged: bool,
    unstaged: bool,
    /// `git show-ref -d` output.
    refs: String,
}

impl Side {
    fn read(cwd: &PathBuf, paths: &[String]) -> Side {
        let head = git(cwd, &["rev-parse", "HEAD"]).unwrap_or_default().trim().to_string();
        let head_branch = git(cwd, &["symbolic-ref", "-q", "HEAD"])
            .ok()
            .and_then(|s| s.trim().strip_prefix("refs/heads/").map(String::from));
        let view_head = if paths.is_empty() {
            head.clone()
        } else {
            let mut a = vec!["rev-list", "-1", "HEAD", "--"];
            a.extend(paths.iter().map(String::as_str));
            git(cwd, &a).unwrap_or_default().trim().to_string()
        };
        let worktree = git(cwd, &["rev-parse", "--is-inside-work-tree"]).is_ok_and(|s| s.trim() == "true");
        // checked whether or not view_head is listed; open decides
        let (mut staged, mut unstaged) = (false, false);
        if worktree {
            let mut a = vec!["diff-index", "--cached", "--quiet", "--ignore-submodules=dirty", "HEAD", "--"];
            a.extend(paths.iter().map(String::as_str));
            staged = git_differs(cwd, &a);
            let mut a = vec!["diff-files", "--quiet", "--"];
            a.extend(paths.iter().map(String::as_str));
            unstaged = git_differs(cwd, &a);
        }
        let refs = git(cwd, &["show-ref", "-d"]).unwrap_or_default();
        Side { head, head_branch, view_head, staged, unstaged, refs }
    }
}

/// A history entry: the selected row and the diff pane's top line (fractional).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Visit {
    pub row: usize,
    pub line: f32,
}

/// Back/forward navigation over selected rows.
#[derive(Default, Debug)]
pub struct History {
    items: Vec<Visit>,
    pos: usize,
}

impl History {
    pub fn push(&mut self, row: usize) {
        if self.current().is_some_and(|v| v.row == row) {
            return;
        }
        self.items.truncate(self.pos);
        self.items.push(Visit { row, line: 0.0 });
        self.pos += 1;
    }
    /// Records the diff position of the current entry.
    pub fn set_line(&mut self, line: f32) {
        if let Some(v) = self.pos.checked_sub(1).and_then(|i| self.items.get_mut(i)) {
            v.line = line;
        }
    }
    pub fn current(&self) -> Option<Visit> {
        self.items.get(self.pos.checked_sub(1)?).copied()
    }
    pub fn back(&mut self) -> Option<Visit> {
        if !self.can_back() {
            return None;
        }
        self.pos -= 1;
        self.current()
    }
    pub fn forward(&mut self) -> Option<Visit> {
        if !self.can_forward() {
            return None;
        }
        self.pos += 1;
        self.current()
    }
    pub fn can_back(&self) -> bool {
        self.pos > 1
    }
    pub fn can_forward(&self) -> bool {
        self.pos < self.items.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_dates() {
        let now = "2026-10-26 20:13:00";
        assert_eq!(short_time("2026-10-26 09:05:59", now), "09:05");
        assert_eq!(short_time("2026-03-05 09:05:59", now), "Mar 5");
        assert_eq!(short_time("2024-03-05 09:05:59", now), "2024-03-05");
    }

    #[test]
    fn history() {
        let v = |row| Some(Visit { row, line: 0.0 });
        let mut h = History::default();
        h.push(1);
        h.set_line(4.5);
        h.push(2);
        h.push(2);
        h.push(3);
        assert_eq!(h.back(), v(2));
        assert_eq!(h.back(), Some(Visit { row: 1, line: 4.5 }));
        assert_eq!(h.back(), None);
        assert_eq!(h.forward(), v(2));
        h.set_line(7.0);
        h.push(5);
        assert!(!h.can_forward());
        assert_eq!(h.back(), Some(Visit { row: 2, line: 7.0 }));
        assert_eq!(h.current(), Some(Visit { row: 2, line: 7.0 }));
    }

    #[test]
    fn diff_args_dropped() {
        for a in ["-U1", "--unified=5", "-p", "--stat=80", "--name-only", "--cc", "-m", "--word-diff=color", "--raw"] {
            assert!(breaks_parsing(a), "{a}");
        }
        assert!(!breaks_parsing("--all") && !breaks_parsing("--grep=x"));
        assert!(parse_record("diff --git a/x b/x\x1f\x1f\x1f\x1f\x1f\x1f\x1f\x1f").is_none());
        let id = "a".repeat(40);
        assert!(parse_record(&format!("{id}\x1f{id}\x1fn\x1fe\x1f0\x1fn\x1fe\x1f0\x1fmsg")).is_some());
        let dir = env!("CARGO_MANIFEST_DIR");
        let n = Repo::open(dir, &[]).unwrap().commits.len();
        let r = Repo::open(dir, &["-U1".into(), "--stat".into()]).unwrap();
        assert_eq!(r.commits.len(), n);
        assert!(r.commits.iter().all(|c| is_id(c.id())));
    }

    #[test]
    fn relative_paths_and_reload() {
        // run from core/, so "src" means core/src
        let dir = env!("CARGO_MANIFEST_DIR");
        let r = Repo::open(dir, &["--".into(), "src".into()]).unwrap();
        let want = git(&PathBuf::from(dir), &["log", "--topo-order", "--all", "--format=%H", "--", "src"]).unwrap();
        let got: Vec<&str> = r.commits.iter().filter(|c| !c.is_fake()).map(|c| c.id()).collect();
        assert_eq!(got, want.lines().collect::<Vec<_>>());
        let sel = r.commits.len() - 1;
        let (r2, row) = r.reload(sel).unwrap();
        assert_eq!(r2.commits[row].id(), r.commits[sel].id());
        assert_eq!(r2.commits.len(), r.commits.len());
    }

    #[test]
    fn branch_own_commits() {
        let repo = Repo::open(env!("CARGO_MANIFEST_DIR"), &[]).unwrap();
        let base: Vec<&str> = ["main", "master", "origin/main", "origin/master"].into_iter()
            .filter(|b| git(&repo.dir, &["rev-parse", "-q", "--verify", b]).is_ok()).collect();
        for b in repo.locations().branches {
            let mut args = vec!["rev-list", b.name.as_str(), "--not"];
            args.extend(&base);
            let mut want: Vec<String> = git(&repo.dir, &args).unwrap().lines().map(String::from).collect();
            let mut got = b.own.clone();
            (want.sort(), got.sort());
            assert_eq!(got, want, "{}", b.name);
        }
    }

    #[test]
    fn reload_keeps_place() {
        let dir = std::env::temp_dir().join(format!("gr-reload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sh = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .current_dir(&dir)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgSign=false"])
                .args(args)
                .status()
                .unwrap()
                .success();
            assert!(ok, "{args:?}");
        };
        let subject = |r: &Repo, row: usize| r.commits[row].subject().to_string();
        sh(&["init", "-q", "-b", "main"]);
        for m in ["a", "b", "c"] {
            sh(&["commit", "-q", "--allow-empty", "-m", m]);
        }
        let r = Repo::open(&dir, &[]).unwrap();
        let row = |r: &Repo, s: &str| r.commits.iter().position(|c| c.subject() == s).unwrap();

        // same id
        let (r, sel) = r.reload(row(&r, "b")).unwrap();
        assert_eq!(subject(&r, sel), "b");
        // amended: new id, same subject
        sh(&["commit", "-q", "--amend", "--allow-empty", "--no-edit", "--date=2001-01-01"]);
        let (r, sel) = r.reload(row(&r, "c")).unwrap();
        assert_eq!(subject(&r, sel), "c");
        // rewritten: neither id nor subject, so one below main's tip again
        sh(&["reset", "-q", "--hard", "HEAD~2"]);
        sh(&["commit", "-q", "--allow-empty", "-m", "b2"]);
        sh(&["commit", "-q", "--allow-empty", "-m", "c2"]);
        let (r, sel) = r.reload(row(&r, "b")).unwrap();
        assert_eq!(subject(&r, sel), "b2");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn open_this_repo() {
        let repo = Repo::open(env!("CARGO_MANIFEST_DIR"), &[]).unwrap();
        assert!(!repo.commits.is_empty());
        let last = repo.commits.iter().position(|c| !c.is_fake()).unwrap();
        let d = repo.details(last, &DiffOpts::default());
        assert!(d.lines[0].text.starts_with("Author: "));
        assert_eq!(d.files[0].0, "Comments");
        let short = &repo.commits[last].id()[..8];
        assert_eq!(repo.resolve(short), Ok(last));
        assert_eq!(repo.resolve("HEAD").map(|i| repo.commits[i].id().to_string()).ok(),
                   git(&repo.dir, &["rev-parse", "HEAD"]).ok().map(|s| s.trim().to_string()));
    }
}

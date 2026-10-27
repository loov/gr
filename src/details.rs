use crate::{decorate, git, Commit, Repo, Rgb, INDEX_ID, UNCOMMITTED_ID};

#[derive(Clone, Debug, PartialEq)]
pub struct DiffOpts {
    pub context: u32,
    pub ignore_space: bool,
    /// Mark the words that changed in pairs of removed and added lines.
    pub words: bool,
    /// Colour keywords, strings, comments and numbers.
    pub syntax: bool,
}

impl Default for DiffOpts {
    fn default() -> Self {
        DiffOpts { context: 3, ignore_space: false, words: true, syntax: true }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Header and commit message text.
    Plain,
    /// "Parent:"/"Child:" lines and message lines naming a commit; `Line::link` holds the target row.
    Link,
    /// The "---- path ----" bar that starts each file.
    FileSep,
    /// Extended diff header lines (mode, rename, index, binary).
    FileHeader,
    Hunk,
    Context,
    /// Line removed (in some parent).
    Del,
    /// Line added.
    Add,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    pub fg: Rgb,
    pub bg: Option<Rgb>,
    pub bold: bool,
}

#[derive(Clone, Debug)]
pub struct Line {
    /// Tabs are already expanded.
    pub text: String,
    pub kind: Kind,
    pub style: Style,
    pub link: Option<usize>,
    /// A line of a merge diff, with a prefix column per parent.
    pub merge: bool,
    /// Char ranges of the words that changed, for word diff.
    pub changed: Vec<(usize, usize)>,
    /// Char ranges coloured by syntax.
    pub syntax: Vec<(usize, usize, Rgb)>,
    /// In a side-by-side diff, the right column's line; None spans both.
    pub right: Option<Box<Line>>,
    /// Line numbers in the old and new file, 0 where the line is not in
    /// that side; only for lines of a diff against one parent.
    pub old: u32,
    pub new: u32,
}

#[derive(Clone, Default)]
pub struct Details {
    pub lines: Vec<Line>,
    /// ("Comments", 0) followed by each changed file and the line where its diff starts.
    pub files: Vec<(String, usize)>,
}

impl Details {
    /// Side by side: each removed line next to the added line it pairs with,
    /// a blank where one side has none, unchanged lines on both sides, and
    /// headers and merge diffs across both.
    pub fn split(self) -> Details {
        let blank = || Line {
            text: String::new(),
            kind: Kind::Plain,
            style: plain(),
            link: None,
            merge: false,
            changed: vec![],
            syntax: vec![],
            right: None,
            old: 0,
            new: 0,
        };
        let code: Vec<Option<Kind>> =
            self.lines.iter().map(|l| (!l.merge && matches!(l.kind, Kind::Context | Kind::Add | Kind::Del)).then_some(l.kind)).collect();
        let n = self.lines.len();
        let mut lines: Vec<Option<Line>> = self.lines.into_iter().map(Some).collect();
        // row of each old line, for the file positions
        let mut row_of = vec![0; n + 1];
        let mut out: Vec<Line> = vec![];
        let mut i = 0;
        while i < n {
            let run = |from: usize, k: Kind| code[from..].iter().take_while(|c| **c == Some(k)).count();
            let (dels, adds) = match code[i] {
                Some(Kind::Del) => (run(i, Kind::Del), run(i + run(i, Kind::Del), Kind::Add)),
                Some(Kind::Add) => (0, run(i, Kind::Add)),
                _ => {
                    row_of[i] = out.len();
                    let mut l = lines[i].take().unwrap();
                    if code[i] == Some(Kind::Context) {
                        l.right = Some(Box::new(l.clone()));
                    }
                    out.push(l);
                    i += 1;
                    continue;
                }
            };
            for k in 0..dels.max(adds) {
                let mut left = if k < dels { row_of[i + k] = out.len(); lines[i + k].take().unwrap() } else { blank() };
                let right = if k < adds { row_of[i + dels + k] = out.len(); lines[i + dels + k].take().unwrap() } else { blank() };
                left.right = Some(Box::new(right));
                out.push(left);
            }
            i += dels + adds;
        }
        row_of[n] = out.len();
        let files = self.files.into_iter().map(|(name, l)| (name, row_of[l])).collect();
        Details { lines: out, files }
    }

    /// Index into `files` of the file shown at `line` (for scroll sync).
    pub fn file_at(&self, line: usize) -> usize {
        self.files.iter().rposition(|(_, l)| *l <= line).unwrap_or(0)
    }
}

const BLACK: Rgb = [0, 0, 0];
const MERGE_COLORS: [Rgb; 16] = [
    [255, 0, 0], [0, 0, 255], [0, 255, 0], [160, 32, 240], [165, 42, 42], [0, 0x90, 0x90],
    [255, 0, 255], [0x80, 0x80, 0], [0, 0x90, 0], [0xff, 0, 0x80], [0, 255, 255], [0xb0, 0x70, 0x70],
    [0x70, 0xb0, 0xf0], [0x70, 0xf0, 0xb0], [0xf0, 0xb0, 0x70], [0xff, 0x70, 0xb0],
];
const DARKGREY: Rgb = [0xa9, 0xa9, 0xa9];

fn plain() -> Style {
    Style { fg: BLACK, bg: None, bold: false }
}

fn merge_color(n: usize) -> Rgb {
    MERGE_COLORS.get(n).copied().unwrap_or(DARKGREY)
}

fn style_for(kind: Kind, merge: bool, parent: Option<usize>) -> Style {
    match kind {
        Kind::Plain | Kind::Context => plain(),
        Kind::Link => Style { fg: [0, 0, 255], bg: None, bold: false },
        Kind::FileSep => Style { fg: BLACK, bg: Some([0xaa, 0xaa, 0xaa]), bold: true },
        Kind::FileHeader => Style { fg: BLACK, bg: Some([0xaa, 0xaa, 0xaa]), bold: false },
        Kind::Hunk => Style { fg: [0, 0, 255], bg: None, bold: false },
        Kind::Del if merge => Style { fg: merge_color(parent.unwrap_or(0)), bg: None, bold: false },
        Kind::Del => Style { fg: [0xc3, 0, 0], bg: Some([0xff, 0xf3, 0xf3]), bold: false },
        Kind::Add if merge => Style { fg: parent.map(merge_color).unwrap_or(BLACK), bg: None, bold: true },
        Kind::Add => Style { fg: [0, 0x98, 0], bg: Some([0xf0, 0xff, 0xf0]), bold: false },
    }
}

fn expand_tabs(s: &str, prefix: usize) -> String {
    if !s.contains('\t') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 16);
    let mut col = 0usize;
    for (i, ch) in s.chars().enumerate() {
        if ch == '\t' {
            // tab stops are relative to the end of the diff prefix, like gitk
            let rel = col.saturating_sub(prefix.min(i));
            let n = 8 - rel % 8;
            out.extend(std::iter::repeat_n(' ', n));
            col += n;
        } else {
            out.push(ch);
            col += 1;
        }
    }
    out
}

struct Builder {
    lines: Vec<Line>,
    files: Vec<(String, usize)>,
}

impl Builder {
    fn push(&mut self, text: impl Into<String>, kind: Kind) {
        self.push_styled(text.into(), kind, style_for(kind, false, None), None);
    }
    fn push_styled(&mut self, text: String, kind: Kind, style: Style, link: Option<usize>) {
        self.lines.push(Line { text, kind, style, link, merge: false, changed: vec![], syntax: vec![], right: None, old: 0, new: 0 });
    }
    fn link(&mut self, text: String, row: Option<usize>) {
        let kind = if row.is_some() { Kind::Link } else { Kind::Plain };
        self.push_styled(text, kind, style_for(kind, false, None), row);
    }
    fn file_sep(name: &str) -> String {
        let pad = "-".repeat(78usize.saturating_sub(name.len()) / 2);
        format!("{pad} {name} {pad}")
    }
    fn start_file(&mut self, name: &str) {
        self.push("", Kind::Plain);
        self.files.push((name.to_string(), self.lines.len()));
        self.push(Self::file_sep(name), Kind::FileSep);
    }
    fn rename_file(&mut self, name: &str) {
        if let Some((n, l)) = self.files.last_mut() {
            *n = name.to_string();
            self.lines[*l].text = Self::file_sep(name);
        }
    }
}

/// "a/x b/x" -> "x" when both sides match (names may contain spaces);
/// quoted names come first and are taken as is. A guess for renames, which
/// the "rename to" / "+++" lines correct.
fn git_diff_name(rest: &str) -> String {
    if rest.starts_with('"') {
        let (a, _) = split_quoted(rest);
        return a.strip_prefix("a/").unwrap_or(&a).to_string();
    }
    let n = rest.len();
    if n % 2 == 1 && rest.as_bytes()[n / 2] == b' ' {
        // a space byte is always a char boundary
        let (a, b) = (&rest[..n / 2], &rest[n / 2 + 1..]);
        if let (Some(a), Some(b)) = (a.strip_prefix("a/"), b.strip_prefix("b/")) {
            if a == b {
                return a.to_string();
            }
        }
    }
    rest.strip_prefix("a/").unwrap_or(rest).to_string()
}

/// Decodes a C-style quoted path (`"\303\251"` -> "é"); unquoted input is returned as is.
fn unquote(s: &str) -> String {
    if s.starts_with('"') { split_quoted(s).0 } else { s.to_string() }
}

/// Parses a leading quoted string, returning it decoded and the rest of the input.
fn split_quoted(s: &str) -> (String, &str) {
    let b = s.as_bytes();
    let mut out = vec![];
    let mut i = 1;
    while i < b.len() && b[i] != b'"' {
        if b[i] == b'\\' && i + 1 < b.len() {
            i += 1;
            match b[i] {
                b'0'..=b'7' => {
                    let end = (i + 3).min(b.len());
                    let oct = std::str::from_utf8(&b[i..end]).unwrap_or("0");
                    out.push(u8::from_str_radix(oct, 8).unwrap_or(b'?'));
                    i = end;
                    continue;
                }
                c => out.push(match c {
                    b'a' => 7,
                    b'b' => 8,
                    b'f' => 12,
                    b'n' => b'\n',
                    b'r' => b'\r',
                    b't' => b'\t',
                    b'v' => 11,
                    c => c, // \\ and \"
                }),
            }
        } else {
            out.push(b[i]);
        }
        i += 1;
    }
    (String::from_utf8_lossy(&out).into_owned(), s.get(i + 1..).unwrap_or(""))
}

/// Name from a `--submodule=log` summary line ("Submodule sub abc..def:" or "Submodule sub contains ...").
fn submodule_name(rest: &str) -> &str {
    let end = rest.find(" contains ").unwrap_or_else(|| rest.rfind(' ').unwrap_or(rest.len()));
    &rest[..end]
}

fn parse_diff(b: &mut Builder, diff: &str) {
    let mut in_hdr = false;
    let mut nparents = 1usize;
    // next old and new line numbers in a single-parent hunk
    let (mut old, mut new) = (0u32, 0u32);
    let mut submodule = String::new();
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("Submodule ") {
            let name = submodule_name(rest);
            if submodule != name {
                b.start_file(name);
                submodule = name.to_string();
            }
            in_hdr = false;
            b.push(line, Kind::FileHeader);
            continue;
        }
        if !submodule.is_empty() && (line.starts_with("  >") || line.starts_with("  <")) {
            let kind = if line.starts_with("  >") { Kind::Add } else { Kind::Del };
            b.push(line, kind);
            continue;
        }
        if line.starts_with("diff ") {
            submodule.clear();
        }
        if let Some(rest) = line.strip_prefix("diff --git ") {
            b.start_file(&git_diff_name(rest));
            in_hdr = true;
            nparents = 1;
        } else if let Some(rest) = line.strip_prefix("diff --cc ").or_else(|| line.strip_prefix("diff --combined ")) {
            b.start_file(&unquote(rest));
            in_hdr = true;
        } else if line.starts_with("@@") {
            nparents = line.bytes().take_while(|&c| c == b'@').count().saturating_sub(1).max(1);
            in_hdr = false;
            // "@@ -12,3 +14,5 @@"
            let start = |sign: char| -> u32 {
                let f = line.split(' ').find_map(|w| w.strip_prefix(sign)).unwrap_or("0");
                f.split(',').next().and_then(|n| n.parse().ok()).unwrap_or(0)
            };
            (old, new) = (start('-'), start('+'));
            b.push(line, Kind::Hunk);
        } else if in_hdr {
            if let Some(n) = line.strip_prefix("rename to ").or_else(|| line.strip_prefix("copy to ")) {
                b.rename_file(&unquote(n));
            }
            if line.starts_with("--- ") {
                continue;
            }
            if let Some(n) = line.strip_prefix("+++ ") {
                let n = unquote(n.trim_end_matches('\t'));
                if n != "/dev/null" {
                    b.rename_file(n.strip_prefix("b/").unwrap_or(&n));
                }
                in_hdr = false;
                continue;
            }
            b.push(line, Kind::FileHeader);
        } else {
            let prefix: String = line.chars().take(nparents).collect();
            let text = expand_tabs(line, nparents);
            let merge = nparents > 1;
            if prefix.len() == nparents && prefix.chars().all(|c| matches!(c, ' ' | '+' | '-')) || line.is_empty() {
                let (kind, parent) = if let Some(k) = prefix.find('-') {
                    (Kind::Del, Some(k))
                } else if prefix.contains('+') {
                    (Kind::Add, prefix.find(' '))
                } else {
                    (Kind::Context, None)
                };
                b.push_styled(text, kind, style_for(kind, merge, parent), None);
                let l = b.lines.last_mut().unwrap();
                l.merge = merge;
                if !merge {
                    if kind != Kind::Add {
                        l.old = old;
                        old += 1;
                    }
                    if kind != Kind::Del {
                        l.new = new;
                        new += 1;
                    }
                }
            } else {
                // "\ No newline at end of file", submodule summaries, ...
                b.push(text, Kind::Hunk);
            }
        }
    }
}

impl Repo {
    /// Text for the details pane: commit header, message and diff.
    // Runs git synchronously on selection; move to a thread if huge diffs stall the UI.
    pub fn details(&self, row: usize, opts: &DiffOpts) -> Details {
        let c = &self.commits[row];
        let mut b = Builder { lines: vec![], files: vec![("Comments".into(), 0)] };
        let ctx = format!("-U{}", opts.context);
        let mut diff_args: Vec<&str> = vec!["-p", "--textconv", "--submodule", "-C", &ctx];
        if opts.ignore_space {
            diff_args.push("-w");
        }

        let mut args = if c.id() == UNCOMMITTED_ID {
            vec!["diff-files"]
        } else if c.id() == INDEX_ID {
            vec!["diff-index", "--cached", "--ignore-submodules=dirty"]
        } else {
            vec!["diff-tree", "-r", "--cc", "--no-commit-id", "--root"]
        };
        args.extend(&diff_args);
        match c.id() {
            UNCOMMITTED_ID => {}
            INDEX_ID => args.push("HEAD"),
            id => args.push(id),
        }
        args.push("--");
        args.extend(self.paths.iter().map(String::as_str));
        // the diff runs while the header (tags, branches) is gathered
        let cwd = &self.cwd;
        let diff = std::thread::scope(|s| {
            let diff = s.spawn(|| git(cwd, &args));
            if c.is_fake() {
                b.push(format!("    {}", c.subject()), Kind::Plain);
            } else {
                self.header(&mut b, c);
            }
            diff.join().unwrap()
        });
        match diff {
            Ok(d) => parse_diff(&mut b, &d),
            Err(e) => b.push(format!("Error getting diffs: {e}"), Kind::Hunk),
        }
        if opts.words {
            decorate::word_diff(&mut b.lines);
        }
        if opts.syntax {
            decorate::syntax(&mut b.lines, &b.files);
        }
        Details { lines: b.lines, files: b.files }
    }

    /// Author, parents, children, branches, tags and the message.
    fn header(&self, b: &mut Builder, c: &Commit) {
        b.push(format!("Author: {}  {}", c.author(), c.author_date()), Kind::Plain);
        b.push(format!("Committer: {}  {}", c.committer(), c.committer_date()), Kind::Plain);
        let tags: Vec<&str> = c.refs.iter().filter(|r| r.kind == crate::RefKind::Tag).map(|r| r.name.as_str()).collect();
        if !tags.is_empty() {
            b.push(format!("Tags: {}", tags.join(" ")), Kind::Plain);
        }
        for p in c.parents() {
            let pi = self.index_of(p);
            let subj = pi.map(|i| self.commits[i].subject()).unwrap_or("...");
            b.link(format!("Parent: {p} ({subj})"), pi);
        }
        for &k in &c.children {
            let kc = &self.commits[k];
            b.link(format!("Child:  {} ({})", kc.id(), kc.subject()), Some(k));
        }
        let near = self.near.borrow_mut().entry(c.id().to_string()).or_insert_with(|| self.near_refs(c.id())).clone();
        for (label, names) in near {
            b.push(format!("{label}: {names}"), Kind::Plain);
        }
        b.push("", Kind::Plain);
        // A whole message line links to the first commit it names by a
        // 6-40 char unique id prefix; per-span links if lines with several IDs matter.
        // Each candidate word is a linear prefix scan, so at most 64 per message;
        // a sorted id index would lift the cap.
        let mut budget = 64;
        for l in c.body().lines() {
            let row = l
                .split(|ch: char| !ch.is_ascii_alphanumeric())
                .filter(|w| (6..=40).contains(&w.len()) && w.chars().all(|ch| ch.is_ascii_hexdigit()))
                .take_while(|_| budget > 0 && { budget -= 1; true })
                .find_map(|w| self.resolve(w).ok());
            b.link(format!("    {}", expand_tabs(l, 0)), row);
        }
    }

    /// Branch / Follows / Precedes lines.
    // gitk walks the whole graph once and lists every nearest tag; this asks
    // git per selection and shows one tag each way.
    fn near_refs(&self, id: &str) -> Vec<(&'static str, String)> {
        let mut out = vec![];
        let fmt = "--format=%(refname)";
        let branches = match self.reach().and_then(|r| r.branches_containing(id)) {
            Some(names) => Some(list(&names)),
            None => git(&self.cwd, &["for-each-ref", fmt, "--contains", id, "refs/heads", "refs/remotes"]).ok().map(|b| {
                let names: Vec<&str> = b
                    .lines()
                    .filter(|l| !l.ends_with("/HEAD"))
                    .map(|l| l.strip_prefix("refs/heads/").unwrap_or_else(|| l.trim_start_matches("refs/")))
                    .collect();
                list(&names)
            }),
        };
        if let Some((names, n)) = branches {
            out.push((if n > 1 { "Branches" } else { "Branch" }, names));
        }
        // nearest tag below each parent (not just the first one) and the
        // nearest one above: from the in-memory graph, else from git, with
        // the git calls in parallel
        let cwd = &self.cwd;
        let reach = self.reach();
        let precedes = reach.and_then(|r| r.precedes(id)).map(String::from);
        let follows: Option<Vec<String>> =
            reach.and_then(|r| Some(r.parents_of(id)?.into_iter().filter_map(|p| r.describe(p)).map(String::from).collect()));
        let (tags, precedes) = std::thread::scope(|s| {
            let precedes = s.spawn(|| {
                precedes.unwrap_or_else(|| {
                    let p = git(cwd, &["name-rev", "--tags", "--name-only", "--no-undefined", id]).unwrap_or_default();
                    p.trim().split(['~', '^']).next().unwrap_or("").trim_start_matches("tags/").to_string()
                })
            });
            let tags = follows.unwrap_or_else(|| {
                let parents = git(cwd, &["rev-parse", &format!("{id}^@")]).unwrap_or_default();
                let tags: Vec<_> = parents
                    .lines()
                    .map(|p| {
                        let p = p.to_string();
                        s.spawn(move || git(cwd, &["describe", "--tags", "--abbrev=0", &p]))
                    })
                    .collect();
                tags.into_iter().filter_map(|t| t.join().unwrap().ok()).map(|t| t.trim().to_string()).collect()
            });
            (tags, precedes.join().unwrap())
        });
        let mut follows: Vec<String> = vec![];
        for t in tags {
            if !follows.contains(&t) {
                follows.push(t);
            }
        }
        out.push(("Follows", follows.join(", ")));
        out.push(("Precedes", precedes));
        out
    }
}

/// The names joined, or "many (n)"; also returns the count.
fn list(names: &[&str]) -> (String, usize) {
    const MAX: usize = 20;
    let s = if names.len() > MAX { format!("many ({})", names.len()) } else { names.join(", ") };
    (s, names.len())
}

#[cfg(test)]
mod tests {
    #[test]
    fn split_pairs_lines() {
        let mut b = Builder { lines: vec![], files: vec![("Comments".into(), 0)] };
        b.start_file("f");
        for (t, k) in [("@@", Kind::Hunk), (" a", Kind::Context), ("-b", Kind::Del), ("-c", Kind::Del), ("+B", Kind::Add), (" d", Kind::Context), ("+e", Kind::Add)] {
            b.push(t, k);
        }
        let d = Details { lines: b.lines, files: b.files }.split();
        let rows: Vec<(String, Option<String>)> = d.lines.iter().map(|l| (l.text.clone(), l.right.as_ref().map(|r| r.text.clone()))).collect();
        let s = |a: &str, b: Option<&str>| (a.to_string(), b.map(String::from));
        assert_eq!(rows, [
            s("", None),
            s(&Builder::file_sep("f"), None),
            s("@@", None),
            s(" a", Some(" a")),
            s("-b", Some("+B")),
            s("-c", Some("")),
            s(" d", Some(" d")),
            s("", Some("+e")),
        ]);
        assert_eq!(d.files, [("Comments".to_string(), 0), ("f".to_string(), 1)]);
    }

    use super::*;

    fn build(diff: &str) -> Builder {
        let mut b = Builder { lines: vec![], files: vec![("Comments".into(), 0)] };
        parse_diff(&mut b, diff);
        b
    }

    #[test]
    fn simple_diff() {
        let b = build(
            "diff --git a/my file.txt b/my file.txt\nindex 1..2 100644\n--- a/my file.txt\n+++ b/my file.txt\n@@ -1,2 +1,2 @@\n ctx\n-old\n+new\n\tx\n",
        );
        assert_eq!(b.files[1].0, "my file.txt");
        let kinds: Vec<Kind> = b.lines.iter().map(|l| l.kind).collect();
        use Kind::*;
        assert_eq!(kinds, [Plain, FileSep, FileHeader, Hunk, Context, Del, Add, Hunk]);
        assert_eq!(b.lines[b.files[1].1].kind, FileSep);
        // a line starting with a tab is not a valid diff line; tab expanded
        assert_eq!(b.lines[7].text, "        x");
    }

    #[test]
    fn quoted_and_submodule() {
        let b = build(
            "diff --git \"a/\\303\\251 x\" \"b/\\303\\251 x\"\nnew file mode 100644\nSubmodule sub 1234567..89abcde:\n  > update\n  < old\n",
        );
        assert_eq!(b.files[1].0, "\u{e9} x");
        assert_eq!(b.files[2].0, "sub");
        let n = b.lines.len();
        assert_eq!((b.lines[n - 2].kind, b.lines[n - 1].kind), (Kind::Add, Kind::Del));
        assert_eq!(unquote("\"tab\\there\""), "tab\there");
    }

    #[test]
    fn rename_and_merge() {
        let b = build(
            "diff --git a/a b/b\nsimilarity index 90%\nrename from a\nrename to b\ndiff --cc m.txt\nindex 1,2..3\n--- a/m.txt\n+++ b/m.txt\n@@@ -1,1 -1,1 +1,1 @@@\n- one\n +two\n++both\n",
        );
        assert_eq!(b.files[1].0, "b");
        assert!(b.lines[b.files[1].1].text.contains(" b "));
        assert_eq!(b.files[2].0, "m.txt");
        let n = b.lines.len();
        assert_eq!((b.lines[n - 3].kind, b.lines[n - 3].style.fg), (Kind::Del, merge_color(0)));
        assert_eq!((b.lines[n - 2].kind, b.lines[n - 2].style.fg), (Kind::Add, merge_color(0)));
        assert!(b.lines[n - 1].style.bold);
    }

    #[test]
    fn utf8_names() {
        // core.quotePath=false: the midpoint guess must not split a char
        assert_eq!(git_diff_name("a/x b/ 中中"), "x b/ 中中");
        let b = build("diff --git a/x b/ 中中\nrename from x\nrename to  中中\n");
        assert_eq!(b.files[1].0, " 中中");
        let b = build("diff --git a/a b b/a b\n--- a/a b\t\n+++ b/a b\t\n@@ -1 +1 @@\n-x\n+y\n");
        assert_eq!(b.files[1].0, "a b");
    }

    #[test]
    fn unquote_escapes() {
        for (q, want) in [
            (r#""\a\b\f\n\r\t\v""#, "\x07\x08\x0c\n\r\t\x0b"),
            (r#""\\ \"x\"""#, "\\ \"x\""),
            (r#""\303\251\101""#, "\u{e9}A"),
            ("plain", "plain"),
        ] {
            assert_eq!(unquote(q), want, "{q}");
        }
    }

    #[test]
    fn follows_all_parents() {
        let dir = std::env::temp_dir().join(format!("gitk-core-follows-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sh = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .current_dir(&dir)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "tag.gpgSign=false", "-c", "commit.gpgSign=false"])
                .args(args)
                .status()
                .unwrap()
                .success();
            assert!(ok, "{args:?}");
        };
        sh(&["init", "-q", "-b", "main"]);
        sh(&["commit", "-q", "--allow-empty", "-m", "root"]);
        sh(&["checkout", "-q", "-b", "side"]);
        sh(&["commit", "-q", "--allow-empty", "-m", "s"]);
        sh(&["tag", "t-side"]);
        sh(&["checkout", "-q", "main"]);
        sh(&["commit", "-q", "--allow-empty", "-m", "m"]);
        sh(&["tag", "t-main"]);
        sh(&["merge", "-q", "--no-ff", "-m", "merge", "side"]);
        let repo = Repo::open(&dir, &[]).unwrap();
        let head = repo.head_row.unwrap();
        let d = repo.details(head, &DiffOpts::default());
        assert!(d.lines.iter().any(|l| l.text == "Follows: t-main, t-side"));
        assert!(repo.near.borrow().contains_key(repo.commits[head].id()));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

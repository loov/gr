use std::ops::Range;

use regex::{Regex, RegexBuilder};

use crate::{Commit, Repo};

/// Which way `Repo::find` moves through the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// Towards older commits.
    Down,
    Up,
}

/// What `Repo::find` does past the end of the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wrap {
    Around,
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Exact,
    IgnCase,
    Regexp,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Exact, Mode::IgnCase, Mode::Regexp];
    pub fn label(self) -> &'static str {
        match self {
            Mode::Exact => "Exact",
            Mode::IgnCase => "IgnCase",
            Mode::Regexp => "Regexp",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Field {
    #[default]
    All,
    Headline,
    Comments,
    Author,
    Committer,
}

impl Field {
    pub const ALL: [Field; 5] = [Field::All, Field::Headline, Field::Comments, Field::Author, Field::Committer];
    pub fn label(self) -> &'static str {
        match self {
            Field::All => "All fields",
            Field::Headline => "Headline",
            Field::Comments => "Comments",
            Field::Author => "Author",
            Field::Committer => "Committer",
        }
    }
}

/// How a commit matched; gitk bolds the headline, and also the author on `Author`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    No,
    Yes,
    Author,
}

/// A "Find ... containing:" query.
pub struct Query {
    re: Regex,
    field: Field,
}

impl Query {
    /// None for an empty string.
    pub fn new(text: &str, mode: Mode, field: Field) -> Result<Option<Query>, String> {
        if text.is_empty() {
            return Ok(None);
        }
        let pat = if mode == Mode::Regexp { text.to_string() } else { regex::escape(text) };
        let re = RegexBuilder::new(&pat)
            .case_insensitive(mode == Mode::IgnCase)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Some(Query { re, field }))
    }

    pub fn hit(&self, c: &Commit) -> Hit {
        let on = |f: Field| self.field == Field::All || self.field == f;
        if on(Field::Author) && self.re.is_match(c.author()) {
            return Hit::Author;
        }
        let any = (on(Field::Headline) && self.re.is_match(c.subject()))
            || (on(Field::Comments) && self.re.is_match(c.body()))
            || (on(Field::Committer) && self.re.is_match(c.committer()));
        if any { Hit::Yes } else { Hit::No }
    }

    /// Byte ranges of matches in `s`, for drawing yellow match boxes.
    pub fn spans(&self, s: &str) -> Vec<Range<usize>> {
        self.re.find_iter(s).map(|m| m.range()).filter(|r| !r.is_empty()).collect()
    }
}

impl Repo {
    /// Next matching row after `from` (or from the start/end when None),
    /// in direction `dir`.
    pub fn find(&self, q: &Query, from: Option<usize>, dir: Dir, wrap: Wrap) -> Option<usize> {
        let n = self.commits.len();
        if n == 0 {
            return None;
        }
        let n = n as isize;
        let forward = dir == Dir::Down;
        let start = from.map(|f| f as isize).unwrap_or(if forward { -1 } else { n });
        (1..=n)
            .map(|k| if forward { start + k } else { start - k })
            .take_while(|&r| wrap == Wrap::Around || (0..n).contains(&r))
            .map(|r| r.rem_euclid(n) as usize)
            .find(|&r| q.hit(&self.commits[r]) != Hit::No)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(subject: &str, author: &str) -> Commit {
        Commit::new("", "", author, 0, "", 0, subject)
    }

    #[test]
    fn modes() {
        let x = c("Fix a.b crash", "Ann <ann@x>");
        let q = |t, m| Query::new(t, m, Field::All).unwrap().unwrap();
        assert_eq!(q("a.b", Mode::Exact).hit(&x), Hit::Yes);
        assert_eq!(q("a\\.b", Mode::Exact).hit(&x), Hit::No);
        assert_eq!(q("fix", Mode::Exact).hit(&x), Hit::No);
        assert_eq!(q("fix", Mode::IgnCase).hit(&x), Hit::Yes);
        assert_eq!(q("^Fix.*crash$", Mode::Regexp).hit(&x), Hit::Yes);
        assert_eq!(q("ann", Mode::Exact).hit(&x), Hit::Author);
        let h = Query::new("ann", Mode::Exact, Field::Headline).unwrap().unwrap();
        assert_eq!(h.hit(&x), Hit::No);
        assert!(Query::new("(", Mode::Regexp, Field::All).is_err());
        assert_eq!(q("a", Mode::Exact).spans("banana"), [1..2, 3..4, 5..6]);
    }

    #[test]
    fn find_wraps() {
        let repo = Repo {
            dir: ".".into(),
            cwd: ".".into(),
            log_args: vec![],
            near: Default::default(),
            reach_job: Default::default(),
            reach: Default::default(),
            commits: vec![c("x", ""), c("hit", ""), c("y", ""), c("hit", "")],
            head_row: None,
            paths: vec![],
            by_id: Default::default(),
            snapshots: Default::default(),
            tags: Default::default(),
            heads: Default::default(),
        };
        let q = Query::new("hit", Mode::Exact, Field::All).unwrap().unwrap();
        assert_eq!(repo.find(&q, Some(1), Dir::Down, Wrap::Around), Some(3));
        assert_eq!(repo.find(&q, Some(3), Dir::Down, Wrap::Around), Some(1));
        assert_eq!(repo.find(&q, Some(3), Dir::Down, Wrap::Stop), None);
        assert_eq!(repo.find(&q, Some(3), Dir::Up, Wrap::Stop), Some(1));
        assert_eq!(repo.find(&q, None, Dir::Down, Wrap::Around), Some(1));
        assert_eq!(repo.find(&q, None, Dir::Down, Wrap::Stop), Some(1));
        assert_eq!(repo.find(&q, None, Dir::Up, Wrap::Stop), Some(3));
    }
}

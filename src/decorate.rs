//! Colouring inside diff lines: the words a change touched, and a light
//! syntax highlight of the code.

use std::sync::LazyLock;

use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};

use crate::details::{Kind, Line};
use crate::Rgb;

/// Token colours, from gitk's palette so themes map them.
const KEYWORD: Rgb = [160, 32, 240];
const STRING: Rgb = [0, 0x90, 0x90];
const COMMENT: Rgb = [0xa9, 0xa9, 0xa9];
const NUMBER: Rgb = [0xff, 0xa5, 0x00];

/// Marks the changed words in each run of removed lines followed by added
/// ones, pairing them in order. Merge diffs, with a prefix column per
/// parent, are left alone.
pub fn word_diff(lines: &mut [Line]) {
    let mut i = 0;
    while i < lines.len() {
        let dels = lines[i..].iter().take_while(|l| l.kind == Kind::Del && !l.merge).count();
        let adds = lines[i + dels..].iter().take_while(|l| l.kind == Kind::Add && !l.merge).count();
        if dels == 0 || adds == 0 {
            i += dels.max(1);
            continue;
        }
        for k in 0..dels.min(adds) {
            let (a, b) = (i + k, i + dels + k);
            if let Some((da, db)) = changed_words(&lines[a].text, &lines[b].text) {
                lines[a].changed = da;
                lines[b].changed = db;
            }
        }
        i += dels + adds;
    }
}

/// Char ranges of a line.
type Ranges = Vec<(usize, usize)>;

/// Char ranges, past the one-char prefix, that differ between `a` and `b`
/// by words. None when the lines share too little for that to help.
fn changed_words(a: &str, b: &str) -> Option<(Ranges, Ranges)> {
    let (ta, tb) = (tokens(a, 1), tokens(b, 1));
    // ponytail: quadratic LCS, so long lines are skipped; Myers diff if they matter
    if ta.len() * tb.len() > 40_000 {
        return None;
    }
    let (ca, cb): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let tok = |c: &[char], t: (usize, usize)| c[t.0..t.1].iter().collect::<String>();
    let (sa, sb): (Vec<String>, Vec<String>) =
        (ta.iter().map(|&t| tok(&ca, t)).collect(), tb.iter().map(|&t| tok(&cb, t)).collect());
    // lcs[i][j]: common length of sa[i..] and sb[j..]
    let mut lcs = vec![vec![0u32; sb.len() + 1]; sa.len() + 1];
    for i in (0..sa.len()).rev() {
        for j in (0..sb.len()).rev() {
            lcs[i][j] = if sa[i] == sb[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut keep_a, mut keep_b) = (vec![false; sa.len()], vec![false; sb.len()]);
    let (mut i, mut j) = (0, 0);
    while i < sa.len() && j < sb.len() {
        if sa[i] == sb[j] {
            (keep_a[i], keep_b[j]) = (true, true);
            (i, j) = (i + 1, j + 1);
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    // mostly rewritten lines read better without the noise
    let same: usize = ta.iter().zip(&keep_a).filter(|(_, k)| **k).map(|(t, _)| t.1 - t.0).sum();
    if same * 2 < ca.len().saturating_sub(1).min(cb.len().saturating_sub(1)) {
        return None;
    }
    let ranges = |t: &[(usize, usize)], keep: &[bool]| {
        let mut out: Vec<(usize, usize)> = vec![];
        for (&(s, e), _) in t.iter().zip(keep).filter(|(_, k)| !**k) {
            match out.last_mut() {
                Some(last) if last.1 == s => last.1 = e,
                _ => out.push((s, e)),
            }
        }
        out
    };
    Some((ranges(&ta, &keep_a), ranges(&tb, &keep_b)))
}

/// Char ranges of the words, runs of spaces and single other chars in `s`
/// from char `from` on.
fn tokens(s: &str, from: usize) -> Vec<(usize, usize)> {
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            0
        } else if c == ' ' {
            1
        } else {
            2
        }
    };
    let chars: Vec<char> = s.chars().collect();
    let mut out = vec![];
    let mut i = from;
    while i < chars.len() {
        let c = class(chars[i]);
        let mut j = i + 1;
        if c != 2 {
            while j < chars.len() && class(chars[j]) == c {
                j += 1;
            }
        }
        out.push((i, j));
        i = j;
    }
    out
}

/// Code lines coloured per diff. Highlighting runs on the UI thread at
/// about 20µs a line, so past this a huge diff stays plain instead of
/// stalling each selection; move it off the thread if that matters.
const SYNTAX_LINES: usize = 20_000;

/// bat's grammars; syntect's own set lacks TOML, TypeScript and Kotlin.
static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_newlines);

/// Token colour by scope prefix.
static COLOURS: LazyLock<Vec<(Scope, Rgb)>> = LazyLock::new(|| {
    [
        ("comment", COMMENT),
        ("string", STRING),
        ("constant.numeric", NUMBER),
        ("constant.language", KEYWORD),
        ("keyword", KEYWORD),
        ("storage", KEYWORD),
    ]
    .into_iter()
    .map(|(s, c)| (Scope::new(s).expect("valid scope"), c))
    .collect()
});

static OPERATOR: LazyLock<Scope> =
    LazyLock::new(|| Scope::new("keyword.operator").expect("valid scope"));

fn find(path: &str) -> Option<&'static SyntaxReference> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = name.rsplit_once('.').map_or(name, |(_, e)| e);
    // whole names like Makefile are listed as extensions too
    SYNTAXES.find_syntax_by_extension(ext).or_else(|| SYNTAXES.find_syntax_by_extension(name))
}

/// Colours the code lines of each file. The old and new file are parsed
/// as separate streams, so a removed "/*" doesn't comment out added lines.
/// Each hunk starts both afresh, which is wrong inside a long comment or
/// string but right everywhere else.
pub fn syntax(lines: &mut [Line], files: &[(String, usize)]) {
    let mut budget = SYNTAX_LINES;
    for (f, (name, start)) in files.iter().enumerate().skip(1) {
        let Some(syntax) = find(name) else { continue };
        let end = files.get(f + 1).map_or(lines.len(), |n| n.1);
        let (mut old, mut new) = (Stream::new(syntax), Stream::new(syntax));
        for line in &mut lines[*start..end] {
            if line.merge {
                continue;
            }
            if budget == 0 {
                return;
            }
            budget -= 1;
            match line.kind {
                Kind::Context => {
                    old.highlight(&line.text);
                    line.syntax = new.highlight(&line.text);
                }
                Kind::Del => line.syntax = old.highlight(&line.text),
                Kind::Add => line.syntax = new.highlight(&line.text),
                _ => {
                    old = Stream::new(syntax);
                    new = Stream::new(syntax);
                }
            }
        }
    }
}

/// One side of a file's diff, parsed line after line.
struct Stream {
    syntax: &'static SyntaxReference,
    parse: ParseState,
    scopes: ScopeStack,
}

impl Stream {
    fn new(syntax: &'static SyntaxReference) -> Stream {
        Stream { syntax, parse: ParseState::new(syntax), scopes: ScopeStack::new() }
    }

    /// Char ranges of `text` to colour, past its one-char diff prefix.
    fn highlight(&mut self, text: &str) -> Vec<(usize, usize, Rgb)> {
        let code = text.get(1..).unwrap_or("");
        let Ok(ops) = self.parse.parse_line(&format!("{code}\n"), &SYNTAXES) else {
            *self = Stream::new(self.syntax);
            return vec![];
        };
        let mut out: Vec<(usize, usize, Rgb)> = vec![];
        let (mut at, mut ch) = (0, 1);
        let ends = ops.iter().map(|(pos, _)| (*pos).min(code.len())).chain([code.len()]);
        let mut ops = ops.iter();
        for pos in ends {
            if pos > at {
                let n = code[at..pos].chars().count();
                if let Some(c) = self.colour(&code[at..pos]) {
                    match out.last_mut() {
                        Some(last) if last.1 == ch && last.2 == c => last.1 += n,
                        _ => out.push((ch, ch + n, c)),
                    }
                }
                (at, ch) = (pos, ch + n);
            }
            if let Some((_, op)) = ops.next() {
                // a broken grammar leaves the stack as it was
                let _ = self.scopes.apply(op);
            }
        }
        out
    }

    fn colour(&self, piece: &str) -> Option<Rgb> {
        let mut scopes = self.scopes.as_slice().iter().rev();
        // the innermost scope with a known prefix decides
        let (scope, c) = scopes.find_map(|s| {
            COLOURS.iter().find(|(p, _)| p.is_prefix_of(*s)).map(|(_, c)| (*s, *c))
        })?;
        // `+` and `->` stay plain, but word operators like `as` and `in` don't
        if OPERATOR.is_prefix_of(scope) && !piece.starts_with(char::is_alphabetic) {
            return None;
        }
        Some(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words() {
        let (a, b) = changed_words("-let x = old_name(1);", "+let x = new_name(1, 2);").unwrap();
        let pick = |s: &str, r: &[(usize, usize)]| {
            r.iter()
                .map(|&(x, y)| s.chars().skip(x).take(y - x).collect::<String>())
                .collect::<Vec<_>>()
        };
        assert_eq!(pick("-let x = old_name(1);", &a), ["old_name"]);
        assert_eq!(pick("+let x = new_name(1, 2);", &b), ["new_name", ", 2"]);
        assert!(changed_words("-completely different", "+nothing alike here at all").is_none());
    }

    /// The coloured pieces of each line, highlighted in order.
    fn colours(path: &str, lines: &[&str]) -> Vec<Vec<(String, Rgb)>> {
        let mut st = Stream::new(find(path).unwrap());
        let piece =
            |s: &str, (a, b, c): (usize, usize, Rgb)| (s.chars().skip(a).take(b - a).collect(), c);
        lines.iter().map(|s| st.highlight(s).into_iter().map(|r| piece(s, r)).collect()).collect()
    }

    #[test]
    fn rust_tokens() {
        let got = colours("a.rs", &[" let n = 42; // \"x\"", " f(\"a\\\"b\", 'x')"]);
        assert_eq!(
            got[0],
            [("let".into(), KEYWORD), ("42".into(), NUMBER), ("// \"x\"".into(), COMMENT)]
        );
        assert_eq!(got[1], [("\"a\\\"b\"".into(), STRING), ("'x'".into(), STRING)]);
        // a block comment carries over to the next line
        let got = colours("a.rs", &[" x as u8 + 1"]);
        assert_eq!(got[0], [("as".into(), KEYWORD), ("u8".into(), KEYWORD), ("1".into(), NUMBER)]);
        let got = colours("a.rs", &["+/* a", "+ b */ fn"]);
        assert_eq!(got[1], [(" b */".into(), COMMENT), ("fn".into(), KEYWORD)]);
    }
}

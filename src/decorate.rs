//! Colouring inside diff lines: the words a change touched, and a light
//! syntax highlight of the code.

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

struct Lang {
    comment: &'static [&'static str],
    keywords: &'static [&'static str],
    /// Single quotes delimit strings, not Rust lifetimes or chars.
    single_quotes: bool,
}

const C_LIKE: &[&str] = &[
    "break", "case", "char", "const", "continue", "default", "do", "double", "else", "enum",
    "extern", "float", "for", "goto", "if", "inline", "int", "long", "return", "short", "signed",
    "sizeof", "static", "struct", "switch", "typedef", "union", "unsigned", "void", "volatile",
    "while", "class", "namespace", "public", "private", "protected", "template", "typename",
    "virtual", "new", "delete", "this", "true", "false", "nullptr", "auto", "bool",
];

fn lang(path: &str) -> Option<Lang> {
    let ext = path.rsplit_once('.').map_or("", |(_, e)| e);
    Some(match ext {
        "rs" => Lang {
            comment: &["//"],
            keywords: &[
                "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else",
                "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match",
                "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct",
                "super", "trait", "true", "type", "unsafe", "use", "where", "while",
            ],
            single_quotes: false,
        },
        "go" => Lang {
            comment: &["//"],
            keywords: &[
                "break", "case", "chan", "const", "continue", "default", "defer", "else",
                "fallthrough", "for", "func", "go", "goto", "if", "import", "interface", "map",
                "package", "range", "return", "select", "struct", "switch", "type", "var", "nil",
                "true", "false",
            ],
            single_quotes: true,
        },
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "m" | "mm" => {
            Lang { comment: &["//"], keywords: C_LIKE, single_quotes: true }
        }
        "java" | "kt" | "kts" | "scala" | "cs" | "swift" | "dart" => Lang {
            comment: &["//"],
            keywords: &[
                "abstract", "break", "case", "catch", "class", "const", "continue", "default",
                "do", "else", "enum", "extends", "final", "finally", "for", "fun", "func", "if",
                "implements", "import", "interface", "let", "new", "null", "nil", "override",
                "package", "private", "protected", "public", "return", "static", "struct", "super",
                "switch", "this", "throw", "try", "val", "var", "void", "when", "while", "true",
                "false",
            ],
            single_quotes: true,
        },
        "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" => Lang {
            comment: &["//"],
            keywords: &[
                "async", "await", "break", "case", "catch", "class", "const", "continue",
                "default", "delete", "do", "else", "export", "extends", "finally", "for", "from",
                "function", "if", "import", "in", "instanceof", "interface", "let", "new", "null",
                "of", "return", "super", "switch", "this", "throw", "try", "type", "typeof",
                "undefined", "var", "void", "while", "yield", "true", "false",
            ],
            single_quotes: true,
        },
        "py" | "pyi" => Lang {
            comment: &["#"],
            keywords: &[
                "and", "as", "assert", "async", "await", "break", "class", "continue", "def",
                "del", "elif", "else", "except", "finally", "for", "from", "global", "if",
                "import", "in", "is", "lambda", "None", "nonlocal", "not", "or", "pass", "raise",
                "return", "True", "False", "try", "while", "with", "yield",
            ],
            single_quotes: true,
        },
        "rb" => Lang {
            comment: &["#"],
            keywords: &[
                "begin", "class", "def", "do", "else", "elsif", "end", "ensure", "false", "for",
                "if", "module", "nil", "require", "rescue", "return", "self", "then", "true",
                "unless", "until", "when", "while", "yield",
            ],
            single_quotes: true,
        },
        "sh" | "bash" | "zsh" => Lang {
            comment: &["#"],
            keywords: &[
                "case", "do", "done", "elif", "else", "esac", "export", "fi", "for", "function",
                "if", "in", "local", "return", "then", "while",
            ],
            single_quotes: true,
        },
        "toml" | "yaml" | "yml" | "conf" | "ini" => {
            Lang { comment: &["#", ";"], keywords: &["true", "false"], single_quotes: true }
        }
        "sql" => Lang {
            comment: &["--"],
            keywords: &[
                "select", "from", "where", "insert", "into", "values", "update", "set", "delete",
                "create", "table", "index", "join", "left", "right", "inner", "outer", "on", "and",
                "or", "not", "null", "group", "by", "order", "limit", "as", "SELECT", "FROM",
                "WHERE", "INSERT", "INTO", "VALUES", "UPDATE", "SET", "DELETE", "CREATE", "TABLE",
                "INDEX", "JOIN", "ON", "AND", "OR", "NOT", "NULL", "GROUP", "BY", "ORDER", "LIMIT",
                "AS",
            ],
            single_quotes: true,
        },
        _ => return None,
    })
}

/// Colours keywords, strings, comments and numbers in the code lines of
/// each file, taking the language from its name. One line at a time, so a
/// comment or string spanning lines is only coloured on its first.
// ponytail: a per-line tokenizer; a real grammar (tree-sitter, syntect) if multi-line constructs matter
pub fn syntax(lines: &mut [Line], files: &[(String, usize)]) {
    for (f, (name, start)) in files.iter().enumerate().skip(1) {
        let Some(lang) = lang(name) else { continue };
        let end = files.get(f + 1).map_or(lines.len(), |n| n.1);
        for line in &mut lines[*start..end] {
            if matches!(line.kind, Kind::Context | Kind::Add | Kind::Del) && !line.merge {
                line.syntax = highlight(&line.text, 1, &lang);
            }
        }
    }
}

fn highlight(s: &str, from: usize, lang: &Lang) -> Vec<(usize, usize, Rgb)> {
    let c: Vec<char> = s.chars().collect();
    let mut out = vec![];
    let mut i = from;
    let starts = |i: usize, p: &str| p.chars().enumerate().all(|(k, pc)| c.get(i + k) == Some(&pc));
    while i < c.len() {
        if lang.comment.iter().any(|p| starts(i, p)) {
            out.push((i, c.len(), COMMENT));
            break;
        }
        let ch = c[i];
        if ch == '"' || ch == '`' || (ch == '\'' && lang.single_quotes) {
            let mut j = i + 1;
            while j < c.len() && c[j] != ch {
                j += if c[j] == '\\' { 2 } else { 1 };
            }
            let end = (j + 1).min(c.len());
            out.push((i, end, STRING));
            i = end;
        } else if ch.is_ascii_digit()
            && (i == 0 || !(c[i - 1].is_alphanumeric() || c[i - 1] == '_'))
        {
            let j = i + c[i..]
                .iter()
                .take_while(|c| c.is_ascii_alphanumeric() || **c == '.' || **c == '_')
                .count();
            out.push((i, j, NUMBER));
            i = j;
        } else if ch.is_alphabetic() || ch == '_' {
            let j = i + c[i..].iter().take_while(|c| c.is_alphanumeric() || **c == '_').count();
            let word: String = c[i..j].iter().collect();
            if lang.keywords.contains(&word.as_str()) {
                out.push((i, j, KEYWORD));
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
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

    #[test]
    fn rust_tokens() {
        let rs = lang("a.rs").unwrap();
        let s = " let n = 42; // \"x\"";
        let got: Vec<(String, Rgb)> = highlight(s, 1, &rs)
            .into_iter()
            .map(|(a, b, c)| (s.chars().skip(a).take(b - a).collect(), c))
            .collect();
        assert_eq!(
            got,
            [("let".into(), KEYWORD), ("42".into(), NUMBER), ("// \"x\"".into(), COMMENT)]
        );
        assert_eq!(highlight(" f(\"a\\\"b\", 'x')", 1, &rs), [(3, 9, STRING)]);
    }
}

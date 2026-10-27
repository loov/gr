//! Per-commit tags such as "ok" (reviewed), kept in `.git/gr.info`
//! as "<commit id> <tag>" lines.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::PathBuf;

#[derive(Debug, Default)]
pub struct Annotations {
    path: PathBuf,
    tags: BTreeMap<String, BTreeSet<String>>,
}

impl Annotations {
    /// Reads `path`; a missing file means no annotations.
    pub fn load(path: PathBuf) -> io::Result<Annotations> {
        let mut a = Annotations { path, tags: BTreeMap::new() };
        let text = match std::fs::read_to_string(&a.path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e),
        };
        for line in text.lines() {
            if let Some((id, tag)) = line.trim().split_once(' ') {
                a.tags.entry(id.to_string()).or_default().insert(tag.trim().to_string());
            }
        }
        Ok(a)
    }

    pub fn has(&self, id: &str, tag: &str) -> bool {
        self.tags.get(id).is_some_and(|t| t.contains(tag))
    }

    /// Ids carrying `tag`.
    pub fn with_tag<'a>(&'a self, tag: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.tags.iter().filter(move |(_, t)| t.contains(tag)).map(|(id, _)| id.as_str())
    }

    /// The value of `<key> <value>` tags on `id`.
    pub fn value(&self, id: &str, key: &str) -> Option<&str> {
        self.tags.get(id)?.iter().find_map(|t| t.strip_prefix(key)?.strip_prefix(' '))
    }

    /// Sets `<key> <value>` on `id`, or removes it with None, and saves.
    pub fn set_value(&mut self, id: &str, key: &str, value: Option<&str>) -> io::Result<()> {
        let tags = self.tags.entry(id.to_string()).or_default();
        tags.retain(|t| t.strip_prefix(key).is_none_or(|r| !r.starts_with(' ')));
        if let Some(v) = value {
            tags.insert(format!("{key} {v}"));
        }
        if tags.is_empty() {
            self.tags.remove(id);
        }
        self.save()
    }

    /// Adds or removes `tag` on `id` and saves; returns whether it is now set.
    pub fn toggle(&mut self, id: &str, tag: &str) -> io::Result<bool> {
        let tags = self.tags.entry(id.to_string()).or_default();
        let set = tags.insert(tag.to_string()) || !tags.remove(tag);
        if tags.is_empty() {
            self.tags.remove(id);
        }
        self.save()?;
        Ok(set)
    }

    fn save(&self) -> io::Result<()> {
        let mut out = String::new();
        for (id, tags) in &self.tags {
            for t in tags {
                out += &format!("{id} {t}\n");
            }
        }
        // write and rename, so a crash never leaves a half-written file
        let tmp = self.path.with_extension("info.tmp");
        std::fs::write(&tmp, out)?;
        std::fs::rename(tmp, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_and_reload() {
        let dir = std::env::temp_dir().join(format!("gr-annotations-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("gr.info");
        let mut a = Annotations::load(path.clone()).unwrap();
        assert!(!a.has("abc", "ok"));
        assert!(a.toggle("abc", "ok").unwrap());
        assert!(a.toggle("def", "ok").unwrap());
        assert!(!a.toggle("def", "ok").unwrap());
        let b = Annotations::load(path.clone()).unwrap();
        assert!(b.has("abc", "ok") && !b.has("def", "ok"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "abc ok\n");
        let mut b = b;
        b.set_value("abc", "patch-id", Some("p1")).unwrap();
        b.set_value("abc", "patch-id", Some("p2")).unwrap();
        assert_eq!(b.value("abc", "patch-id"), Some("p2"));
        assert_eq!(b.with_tag("ok").collect::<Vec<_>>(), ["abc"]);
        b.set_value("abc", "patch-id", None).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "abc ok\n");
        std::fs::remove_dir_all(dir).unwrap();
    }
}

//! Notices when git changes the repository behind the viewer's back: a
//! commit, checkout, fetch or staging in another program.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::git;

pub struct Watch {
    changed: Arc<AtomicBool>,
}

impl Watch {
    /// Polls HEAD, the index and the refs of the repository at `cwd` every
    /// second; `wake` runs on each change. Edits to work tree files that
    /// are not staged go unnoticed.
    // ponytail: mtime polling; use a file-notify crate if a second's delay or the refs walk matters
    pub fn new(cwd: &Path, wake: impl Fn() + Send + 'static) -> Watch {
        let changed = Arc::new(AtomicBool::new(false));
        let cwd = cwd.to_path_buf();
        let dirs = git(&cwd, &["rev-parse", "--git-dir", "--git-common-dir"]).unwrap_or_default();
        let mut dirs = dirs.lines().map(|d| cwd.join(d.trim()));
        let (dir, common) = (dirs.next(), dirs.next());
        if let (Some(dir), Some(common)) = (dir, common) {
            let flag = changed.clone();
            std::thread::spawn(move || {
                let mut last = fingerprint(&dir, &common);
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                    let now = fingerprint(&dir, &common);
                    if now != last {
                        last = now;
                        flag.store(true, Ordering::Relaxed);
                        wake();
                    }
                }
            });
        }
        Watch { changed }
    }

    /// Whether anything changed since the last call.
    pub fn changed(&self) -> bool {
        self.changed.swap(false, Ordering::Relaxed)
    }
}

fn fingerprint(dir: &Path, common: &Path) -> u64 {
    let mut h = DefaultHasher::new();
    let mut files: Vec<PathBuf> = vec![dir.join("HEAD"), dir.join("index"), common.join("packed-refs")];
    let mut stack = vec![common.join("refs")];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() { stack.push(p) } else { files.push(p) }
        }
    }
    files.sort();
    for f in files {
        let m = std::fs::metadata(&f).ok();
        (&f, m.as_ref().map(|m| m.len()), m.and_then(|m| m.modified().ok())).hash(&mut h);
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_follows_refs() {
        let dir = std::env::temp_dir().join(format!("gr-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("refs/heads")).unwrap();
        std::fs::write(dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        let a = fingerprint(&dir, &dir);
        assert_eq!(a, fingerprint(&dir, &dir));
        std::fs::write(dir.join("refs/heads/main"), "0\n").unwrap();
        assert_ne!(a, fingerprint(&dir, &dir));
        std::fs::remove_dir_all(dir).unwrap();
    }
}

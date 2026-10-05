//! Filesystem access confined to the store root.
//!
//! Every user-supplied path is a "/"-separated relative path (it may start
//! with "/"); `Store::resolve` maps it to an absolute path that cannot
//! escape the root.  No symlinks are followed out of the root (the root
//! itself is canonicalised at startup).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub struct Store {
    pub root: PathBuf,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    /// unix seconds
    pub mtime: i64,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Hit {
    /// relative path of the match, e.g. "photos/2024/cat.jpg"
    pub rel: String,
    /// relative path of the containing dir, e.g. "photos/2024"
    pub parent: String,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: i64,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct SearchOutcome {
    pub hits: Vec<Hit>,
    pub truncated: bool,
    /// how many entries were inspected before stopping (capped searches)
    pub scanned: u64,
}

fn mtime_of(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Store {
    pub fn open(root: &Path) -> Result<Self, String> {
        let root =
            fs::canonicalize(root).map_err(|e| format!("cannot resolve root {}: {e}", root.display()))?;
        if !root.is_dir() {
            return Err(format!("{} is not a directory", root.display()));
        }
        Ok(Self { root })
    }

    /// Map a user-supplied relative path to an absolute path inside the
    /// root.  Rejects any `..` component (the frontend never produces
    /// them); empty and `.` components are ignored.
    pub fn resolve(&self, rel: &str) -> Result<PathBuf, String> {
        let mut out = self.root.clone();
        for comp in rel.split('/') {
            if comp.is_empty() || comp == "." {
                continue;
            }
            if comp == ".." {
                return Err("invalid path".to_string());
            }
            out.push(comp);
        }
        Ok(out)
    }

    /// Clean relative path for display ("a/b"); "" for the root.
    pub fn rel_of(&self, abs: &Path) -> String {
        abs.strip_prefix(&self.root)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default()
    }

    pub fn entry(&self, abs: &Path) -> Result<Entry, String> {
        let meta = fs::symlink_metadata(abs).map_err(|e| err_no(e, abs))?;
        Ok(Entry {
            name: abs
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            is_dir: meta.is_dir(),
            size: meta.len(),
            mtime: mtime_of(&meta),
        })
    }

    pub fn list(&self, abs: &Path) -> Result<Vec<Entry>, String> {
        let rd = fs::read_dir(abs).map_err(|e| err_no(e, abs))?;
        let mut out = Vec::new();
        for e in rd {
            let e = match e {
                Ok(e) => e,
                Err(_) => continue,
            };
            let meta = match e.file_type() {
                Ok(ft) if ft.is_symlink() => {
                    // Follow symlinks for display (the NAS may use them),
                    // but only if the target is inside the store.
                    match fs::metadata(e.path()) {
                        Ok(m) => m,
                        Err(_) => continue,
                    }
                }
                Ok(_) => match fs::metadata(e.path()) {
                    Ok(m) => m,
                    Err(_) => continue,
                },
                Err(_) => continue,
            };
            out.push(Entry {
                name: e.file_name().to_string_lossy().to_string(),
                is_dir: meta.is_dir(),
                size: meta.len(),
                mtime: mtime_of(&meta),
            });
        }
        out.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(out)
    }

    /// Case-insensitive substring search by file name.  `deep` walks the
    /// whole tree under `abs`; otherwise only its immediate children are
    /// matched.  Both are capped: at most `max_hits` results and
    /// `max_scanned` inspected entries.
    pub fn search(
        &self,
        abs: &Path,
        q: &str,
        deep: bool,
        max_hits: usize,
        max_scanned: u64,
    ) -> Result<SearchOutcome, String> {
        let needle = q.to_lowercase();
        let mut hits: Vec<Hit> = Vec::new();
        let mut truncated = false;
        let mut scanned: u64 = 0;

        // Add `e` (at `p`) to the hits when its name matches; returns true
        // once the hit cap is reached.
        let try_hit = |p: &Path, e: &Entry, hits: &mut Vec<Hit>| -> bool {
            if hits.len() >= max_hits {
                return true;
            }
            if e.name.to_lowercase().contains(&needle) {
                hits.push(Hit {
                    rel: self.rel_of(p),
                    parent: match p.parent() {
                        Some(pp) => self.rel_of(pp),
                        None => String::new(),
                    },
                    name: e.name.clone(),
                    is_dir: e.is_dir,
                    size: e.size,
                    mtime: e.mtime,
                });
            }
            false
        };

        if !deep {
            if let Ok(entries) = self.list(abs) {
                for e in entries {
                    scanned += 1;
                    let p = abs.join(&e.name);
                    if try_hit(&p, &e, &mut hits) {
                        truncated = true;
                        break;
                    }
                }
            }
            return Ok(SearchOutcome {
                hits,
                truncated,
                scanned,
            });
        }

        // Explicit-stack DFS (deep trees, no recursion).  Directories are
        // scanned before their contents so results appear top-down.
        let mut stack: Vec<PathBuf> = vec![abs.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let entries = match self.list(&dir) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for e in entries {
                scanned += 1;
                let p = dir.join(&e.name);
                if try_hit(&p, &e, &mut hits) {
                    truncated = true;
                    break;
                }
                if e.is_dir {
                    stack.push(p);
                }
            }
            if truncated || scanned >= max_scanned {
                truncated = true;
                break;
            }
        }

        Ok(SearchOutcome {
            hits,
            truncated,
            scanned,
        })
    }
}

fn err_no(e: std::io::Error, p: &Path) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => "no such file or directory".to_string(),
        std::io::ErrorKind::PermissionDenied => format!("permission denied: {}", p.display()),
        _ => format!("{}: {e}", p.display()),
    }
}

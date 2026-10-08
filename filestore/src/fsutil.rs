//! Filesystem access confined to the store root.
//!
//! Every user-supplied path is a "/"-separated relative path (it may start
//! with "/"); `Store::resolve` maps it to an absolute path that cannot
//! escape the root: it rejects any `..` component and the root itself is
//! canonicalised at startup.
//!
//! What that does *not* cover is symlinks.  A link inside the share is
//! followed wherever it points — deliberately for listings (`list` shows the
//! target's size/mtime because the NAS uses links), and unavoidably for reads,
//! since the OS follows the last component when the file is opened.  So the
//! guarantee is "no path traversal from the API", not "nothing outside the
//! share is reachable": creating the link needs write access to the share
//! itself, which is not something this service offers (there is no symlink
//! endpoint).  Pinned by `list_follows_symlinks_that_resolve_even_outside_the_root`.

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
        let root = fs::canonicalize(root)
            .map_err(|e| format!("cannot resolve root {}: {e}", root.display()))?;
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

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A fresh empty directory under the temp dir (no external dep; these
    /// tests are the only thing that needs one).
    fn tmp_store() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "filestore-fsutil-test-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            n
        ));
        fs::create_dir_all(&dir).expect("create temp store");
        dir
    }

    /// root/
    ///   photos/            2024/{cat.jpg, dog.PNG}, note.txt
    ///   top.txt
    ///   Zebra/
    fn fixture() -> Store {
        let root = tmp_store();
        fs::create_dir_all(root.join("photos/2024")).unwrap();
        fs::create_dir_all(root.join("Zebra")).unwrap();
        fs::write(root.join("photos/2024/cat.jpg"), b"cat").unwrap();
        fs::write(root.join("photos/2024/dog.PNG"), b"dog").unwrap();
        fs::write(root.join("photos/note.txt"), b"note").unwrap();
        fs::write(root.join("top.txt"), b"top").unwrap();
        Store::open(&root).unwrap()
    }

    // ---- resolve: the containment guard -----------------------------------

    #[test]
    fn resolve_maps_relative_paths_under_the_root() {
        let s = fixture();
        assert_eq!(s.resolve("").unwrap(), s.root);
        assert_eq!(s.resolve("/").unwrap(), s.root);
        assert_eq!(s.resolve("photos").unwrap(), s.root.join("photos"));
        // leading, trailing and doubled separators, and "." components, are
        // all ignored -- the frontend sends "/photos/2024".
        assert_eq!(
            s.resolve("/photos//2024/").unwrap(),
            s.root.join("photos/2024")
        );
        assert_eq!(
            s.resolve("photos/./2024").unwrap(),
            s.root.join("photos/2024")
        );
        // dots inside a name are not a parent directory
        assert_eq!(s.resolve("..hidden").unwrap(), s.root.join("..hidden"));
        assert_eq!(s.resolve("a...b").unwrap(), s.root.join("a...b"));
        assert_eq!(s.resolve("...").unwrap(), s.root.join("..."));
    }

    #[test]
    fn resolve_rejects_every_parent_directory_component() {
        let s = fixture();
        for bad in [
            "..",
            "../",
            "/..",
            "./..",
            "photos/..",
            "photos/../top.txt",
            "photos/../..",
            "/photos/2024/../../..",
            "a/b/c/..",
        ] {
            assert_eq!(
                s.resolve(bad).err(),
                Some("invalid path".to_string()),
                "accepted {bad:?}"
            );
        }
    }

    #[test]
    fn rel_of_round_trips_relative_paths() {
        let s = fixture();
        assert_eq!(s.rel_of(&s.root), "");
        assert_eq!(s.rel_of(&s.resolve("photos/2024").unwrap()), "photos/2024");
        // outside the root there is no relative path
        assert_eq!(s.rel_of(Path::new("/")), "");
    }

    // ---- entry / list ------------------------------------------------------

    #[test]
    fn entry_reports_directories_and_missing_files() {
        let s = fixture();
        let dir = s.entry(&s.resolve("photos").unwrap()).unwrap();
        assert_eq!(dir.name, "photos");
        assert!(dir.is_dir);
        let file = s.entry(&s.resolve("top.txt").unwrap()).unwrap();
        assert_eq!(file.name, "top.txt");
        assert!(!file.is_dir);
        assert_eq!(file.size, 3);
        assert_eq!(
            s.entry(&s.resolve("nope.txt").unwrap()).err(),
            Some("no such file or directory".to_string())
        );
    }

    #[test]
    fn list_puts_directories_first_then_case_insensitive_names() {
        let s = fixture();
        let names: Vec<String> = s
            .list(&s.root)
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, vec!["photos", "Zebra", "top.txt"]);
    }

    #[test]
    fn list_skips_a_dangling_symlink() {
        let s = fixture();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(s.root.join("gone"), s.root.join("broken")).unwrap();
            let names: Vec<String> = s
                .list(&s.root)
                .unwrap()
                .into_iter()
                .map(|e| e.name)
                .collect();
            assert!(!names.contains(&"broken".to_string()), "listed: {names:?}");
        }
    }

    /// What the containment guard does *not* do: a symlink that resolves is
    /// followed for display wherever it points (the NAS uses them), so its
    /// size/mtime come from outside the root.  Pinned deliberately -- the
    /// module doc used to claim symlinks were never followed out of the root,
    /// which is only true of `resolve`, not of the filesystem calls.
    #[test]
    fn list_follows_symlinks_that_resolve_even_outside_the_root() {
        let s = fixture();
        #[cfg(unix)]
        {
            let outside = tmp_store();
            fs::write(outside.join("secret"), b"1234567890").unwrap();
            std::os::unix::fs::symlink(outside.join("secret"), s.root.join("link")).unwrap();
            let entries = s.list(&s.root).unwrap();
            let link = entries
                .iter()
                .find(|e| e.name == "link")
                .expect("link listed");
            assert!(!link.is_dir);
            assert_eq!(link.size, 10, "size came from the target outside the root");
        }
    }

    // ---- search ------------------------------------------------------------

    #[test]
    fn search_is_case_insensitive_on_the_name() {
        let s = fixture();
        let out = s
            .search(&s.resolve("photos/2024").unwrap(), "CAT", false, 20, 1000)
            .unwrap();
        assert_eq!(out.hits.len(), 1);
        assert_eq!(out.hits[0].name, "cat.jpg");
        assert_eq!(out.hits[0].rel, "photos/2024/cat.jpg");
        assert_eq!(out.hits[0].parent, "photos/2024");
        assert!(!out.truncated);
    }

    #[test]
    fn shallow_search_only_matches_immediate_children() {
        let s = fixture();
        let out = s
            .search(&s.resolve("photos").unwrap(), "cat", false, 20, 1000)
            .unwrap();
        assert!(out.hits.is_empty(), "shallow search found {:?}", out.hits);
        let deep = s
            .search(&s.resolve("photos").unwrap(), "cat", true, 20, 1000)
            .unwrap();
        assert_eq!(deep.hits.len(), 1);
        assert_eq!(deep.hits[0].rel, "photos/2024/cat.jpg");
    }

    #[test]
    fn deep_search_matches_directories_too() {
        let s = fixture();
        let out = s.search(&s.root, "zebra", true, 20, 1000).unwrap();
        assert_eq!(out.hits.len(), 1);
        assert!(out.hits[0].is_dir);
        assert_eq!(out.hits[0].rel, "Zebra");
    }

    #[test]
    fn search_hit_cap_marks_the_result_truncated() {
        let s = fixture();
        let out = s.search(&s.root, "", true, 2, 1000).unwrap();
        assert_eq!(out.hits.len(), 2);
        assert!(out.truncated);
    }

    #[test]
    fn search_scan_cap_marks_the_result_truncated() {
        let s = fixture();
        let out = s.search(&s.root, "cat", true, 20, 1).unwrap();
        assert!(out.truncated);
        // The cap is checked once per directory, not per entry: the root's 3
        // entries are all counted, then the walk stops -- it does not descend
        // into photos/ looking for "cat".
        assert_eq!(
            out.scanned, 3,
            "walk did not stop after the first directory"
        );
        assert!(
            out.hits.is_empty(),
            "descended past the cap: {:?}",
            out.hits
        );
    }

    #[test]
    fn search_in_a_missing_directory_is_not_an_error() {
        let s = fixture();
        let out = s
            .search(&s.resolve("nope").unwrap(), "cat", true, 20, 1000)
            .unwrap();
        assert!(out.hits.is_empty());
        assert_eq!(out.scanned, 0);
    }
}

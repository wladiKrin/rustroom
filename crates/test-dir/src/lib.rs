//! A temp directory of its own for every test (M10.7c) — the only place in the workspace's tests
//! that calls `std::env::temp_dir()`; `every_test_temp_path_goes_through_this_crate` holds that.
//!
//! **Layout:** `<temp>/rustroom-tests/<pid>/<tag>-<n>`. The process id keeps two concurrent runs apart
//! (two gates at once); the counter keeps two tests of one run apart (they are threads of one
//! process). Before this crate, some tests used a fixed name, which collided across concurrent gates,
//! and some the process id alone, which collided between two tests of one run — and nothing was ever
//! removed: 3,128 directories, 72 MB, had piled up in a week.
//!
//! **Cleanup:** a [`TempDir`] deletes its directory when dropped. What is kept — [`path`], or a run
//! that crashed — goes at the next run: each run holds a lock on `<pid>/.lock` for its whole life, and
//! the first directory a run makes deletes every other run's root whose lock it can take, because that
//! run has ended. A concurrent gate still holds its lock, so its directories are never touched.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

/// Where every run's root lives.
fn base() -> PathBuf {
    std::env::temp_dir().join("rustroom-tests")
}

/// This run's root, created — with its lock taken and ended runs swept — on first use.
fn run_root() -> &'static Path {
    static ROOT: OnceLock<(PathBuf, File)> = OnceLock::new();
    let (root, _held) = ROOT.get_or_init(|| {
        let base = base();
        let root = base.join(std::process::id().to_string());
        std::fs::create_dir_all(&root).expect("create the run's test root");
        let lock = File::create(root.join(".lock")).expect("create the run's lock file");
        // ⚠ Held for the life of the process: the `File` lives in this static, so the lock goes
        // only when the run ends — which is what tells the next run it may clean up.
        lock.try_lock().expect("take this run's own lock");
        sweep_ended_runs(&base, &root);
        (root, lock)
    });
    root
}

/// Delete every run root under `base` except `keep` whose run has ended: its lock can be taken.
///
/// ⚠ A root with no lock file is left alone unless it is over a day old — it may be a run between
/// creating its directory and its lock. Errors are ignored: cleanup must never fail a test.
pub fn sweep_ended_runs(base: &Path, keep: &Path) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for e in entries.flatten() {
        let root = e.path();
        if root == keep || !root.is_dir() {
            continue;
        }
        let ended = match File::options().write(true).open(root.join(".lock")) {
            // The lock is released as this `File` drops, just before the directory goes.
            Ok(f) => f.try_lock().is_ok(),
            Err(_) => e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age.as_secs() > 24 * 60 * 60),
        };
        if ended {
            let _ = std::fs::remove_dir_all(&root);
        }
    }
}

/// A test's own directory, deleted when dropped.
#[derive(Debug)]
pub struct TempDir {
    path: PathBuf,
    keep: bool,
}

impl TempDir {
    /// A fresh, empty directory named after `tag`, unique in this run and across runs.
    pub fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let path = run_root().join(format!("{tag}-{}", N.fetch_add(1, Ordering::Relaxed)));
        // ⚠ Removed first: a pid is reused across runs, and a crashed run with the same pid could
        // have left this exact name behind before the sweep saw it.
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create a test's temp directory");
        Self { path, keep: false }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A path inside the directory.
    pub fn join(&self, p: impl AsRef<Path>) -> PathBuf {
        self.path.join(p)
    }

    /// Hand the directory over without deleting it on drop — for a fixture that must return a plain
    /// path. The next run's sweep removes it once this run has ended.
    pub fn keep(mut self) -> PathBuf {
        self.keep = true;
        self.path.clone()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// A fresh directory for `tag` that outlives the call — [`TempDir::keep`] in one word.
pub fn path(tag: &str) -> PathBuf {
    TempDir::new(tag).keep()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_directories_with_one_tag_are_two_directories() {
        let a = TempDir::new("same");
        let b = TempDir::new("same");
        assert_ne!(a.path(), b.path());
        assert!(a.path().is_dir() && b.path().is_dir());
        // ⚠ Across threads too: tests are threads of one process.
        let paths: Vec<PathBuf> = (0..8)
            .map(|_| std::thread::spawn(|| path("threads")))
            .map(|h| h.join().unwrap())
            .collect();
        let mut unique = paths.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), paths.len(), "{paths:?}");
        for p in paths {
            let _ = std::fs::remove_dir_all(p);
        }
    }

    #[test]
    fn a_dropped_directory_is_gone_and_a_kept_one_is_not() {
        let d = TempDir::new("drop");
        let p = d.path().to_path_buf();
        std::fs::write(d.join("f"), b"x").unwrap();
        drop(d);
        assert!(!p.exists(), "dropping did not delete {p:?}");
        let kept = path("kept");
        assert!(kept.is_dir(), "a kept directory was deleted");
        let _ = std::fs::remove_dir_all(kept);
    }

    /// ⚠ Both halves, in a base of the test's own: a root whose run has ended (its lock is free) goes;
    /// one whose run is alive (its lock is held) and a young one with no lock at all stay.
    #[test]
    fn the_sweep_removes_ended_runs_and_nothing_else() {
        let base = TempDir::new("sweep-base");
        let root = |name: &str| {
            let r = base.join(name);
            std::fs::create_dir_all(&r).unwrap();
            r
        };
        let ended = root("111");
        File::create(ended.join(".lock")).unwrap();
        let alive = root("222");
        let held = File::create(alive.join(".lock")).unwrap();
        held.try_lock().unwrap();
        let young = root("333");
        let this = root("444");
        sweep_ended_runs(base.path(), &this);
        assert!(!ended.exists(), "an ended run's root survived");
        assert!(alive.exists(), "a live run's root was deleted");
        assert!(young.exists(), "a young root with no lock was deleted");
        assert!(this.exists(), "this run's own root was deleted");
        drop(held);
    }

    #[test]
    fn this_run_holds_its_own_lock() {
        let _ = TempDir::new("lock");
        let other = File::options()
            .write(true)
            .open(run_root().join(".lock"))
            .unwrap();
        assert!(other.try_lock().is_err(), "this run's lock is not held");
    }

    /// The one place outside this crate allowed to name the system temp directory: the export
    /// staging directory, which is production code with its own unique name.
    const ALLOWED: &str = "app/src/batch_export/worker.rs";

    /// Every `.rs` file under `dir`, recursively.
    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                rust_files(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }

    /// Each file under every crate's `src/` and `tests/` that names the system temp directory, as
    /// a path relative to `crates/`. Examples are manual tools and stay out of it.
    fn temp_dir_users() -> (usize, Vec<String>) {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let mut files = Vec::new();
        for c in std::fs::read_dir(crates).unwrap().flatten() {
            rust_files(&c.path().join("src"), &mut files);
            rust_files(&c.path().join("tests"), &mut files);
        }
        let pattern = ["env::", "temp_dir()"].concat();
        let users = files
            .iter()
            .filter(|f| std::fs::read_to_string(f).is_ok_and(|s| s.contains(&pattern)))
            .map(|f| {
                let rel = f.strip_prefix(crates).unwrap();
                rel.to_string_lossy().replace('\\', "/")
            })
            .collect();
        (files.len(), users)
    }

    /// ⚠⚠ A test path taken straight from the system temp directory is the collision this crate
    /// exists to end: a fixed name meets the same name in a concurrent gate, a process-id one meets
    /// another test of the same run. Every test directory comes from here.
    #[test]
    fn no_crate_names_the_temp_directory_but_this_one() {
        let (scanned, users) = temp_dir_users();
        // ⚠ The sample first: a walk that found nothing would pass everything.
        assert!(scanned > 300, "only {scanned} source files found");
        // Positive control: the one allowed site must be found, or the pattern is blind.
        assert!(
            users.iter().any(|u| u == ALLOWED),
            "the sweep did not find {ALLOWED}: {users:?}"
        );
        let offenders: Vec<_> = users
            .iter()
            .filter(|u| *u != ALLOWED && !u.starts_with("test-dir/"))
            .collect();
        assert!(
            offenders.is_empty(),
            "use test_dir::TempDir or test_dir::path instead: {offenders:?}"
        );
    }

    /// The most lines of code and doc comments a source file may hold (M10.7d).
    const CEILING: usize = 1_000;

    /// If a comment, string or char literal starts at `i`, the index just past it.
    fn skip_literal(c: &[char], i: usize) -> Option<usize> {
        let at = |k: usize| c.get(k).copied();
        let ident_before = i > 0 && (c[i - 1].is_alphanumeric() || c[i - 1] == '_');
        if at(i) == Some('/') && at(i + 1) == Some('/') {
            return Some((i..c.len()).find(|&k| c[k] == '\n').unwrap_or(c.len()));
        }
        if at(i) == Some('/') && at(i + 1) == Some('*') {
            let (mut k, mut depth) = (i, 0);
            while k < c.len() {
                if at(k) == Some('/') && at(k + 1) == Some('*') {
                    depth += 1;
                    k += 2;
                } else if at(k) == Some('*') && at(k + 1) == Some('/') {
                    depth -= 1;
                    k += 2;
                    if depth == 0 {
                        return Some(k);
                    }
                } else {
                    k += 1;
                }
            }
            return Some(c.len());
        }
        if ident_before {
            return None;
        }
        // Raw strings: r"..", r#".."#, br"..".
        let r = if at(i) == Some('b') && at(i + 1) == Some('r') {
            Some(i + 2)
        } else if at(i) == Some('r') {
            Some(i + 1)
        } else {
            None
        };
        if let Some(mut k) = r {
            let hashes = (k..c.len()).take_while(|&j| c[j] == '#').count();
            k += hashes;
            if at(k) == Some('"') {
                let mut j = k + 1;
                while j < c.len() {
                    if c[j] == '"' && (j + 1..=j + hashes).all(|h| at(h) == Some('#')) {
                        return Some(j + 1 + hashes);
                    }
                    j += 1;
                }
                return Some(c.len());
            }
        }
        let quote = if at(i) == Some('b') { i + 1 } else { i };
        match at(quote) {
            Some('"') => {
                let mut j = quote + 1;
                while j < c.len() && c[j] != '"' {
                    j += if c[j] == '\\' { 2 } else { 1 };
                }
                Some(j + 1)
            }
            // A char literal, not a lifetime: an escape, or one char and the closing quote.
            Some('\'') if at(quote + 1) == Some('\\') => {
                (quote + 2..c.len()).find(|&j| c[j] == '\'').map(|j| j + 1)
            }
            Some('\'') if at(quote + 2) == Some('\'') => Some(quote + 3),
            _ => None,
        }
    }

    /// The index just past the item whose tokens start at `from`: its `;` at depth zero, or the `}`
    /// that closes its body (unless a `;` follows, as after a struct literal).
    fn item_end(c: &[char], from: usize) -> usize {
        let (mut i, mut depth) = (from, 0i32);
        while i < c.len() {
            if let Some(next) = skip_literal(c, i) {
                i = next;
                continue;
            }
            match c[i] {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' => depth -= 1,
                '}' => {
                    depth -= 1;
                    let next = c[i + 1..].iter().find(|ch| !ch.is_whitespace());
                    if depth == 0 && next != Some(&';') {
                        return i + 1;
                    }
                }
                ';' if depth == 0 => return i + 1,
                _ => {}
            }
            i += 1;
        }
        c.len()
    }

    /// Non-blank lines outside every `#[cfg(test)]` item, wherever it sits — code and doc comments,
    /// not tests. ⚠ Not "up to the first `#[cfg(test)]`": `main.rs` declares a test module at line
    /// 63, and that measure read it as 78 lines of code where it held 555.
    fn code_and_doc_lines(src: &str) -> usize {
        let c: Vec<char> = src.chars().collect();
        let mut keep = vec![true; c.len()];
        let mut i = 0;
        while i < c.len() {
            let line_end = (i..c.len()).find(|&k| c[k] == '\n').unwrap_or(c.len());
            let line: String = c[i..line_end].iter().collect();
            if line.trim_start().starts_with("#[cfg(test)]") {
                let attr = i + line.find("#[cfg(test)]").unwrap() + "#[cfg(test)]".len();
                let end = item_end(&c, attr);
                keep[i..end].fill(false);
                i = end;
            } else {
                i = line_end + 1;
            }
        }
        let kept: String = c
            .iter()
            .zip(&keep)
            .filter(|(_, k)| **k)
            .map(|(ch, _)| *ch)
            .collect();
        kept.lines().filter(|l| !l.trim().is_empty()).count()
    }

    /// Files that are wholly tests: the targets of a `#[cfg(test)] mod name;` declaration.
    fn test_only_files(files: &[PathBuf]) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for f in files {
            let src = std::fs::read_to_string(f).unwrap_or_default();
            let lines: Vec<&str> = src.lines().collect();
            for (i, l) in lines.iter().enumerate() {
                if l.trim() != "#[cfg(test)]" {
                    continue;
                }
                let next = lines.get(i + 1).map_or("", |n| n.trim());
                let decl = ["pub(crate) ", "pub(super) ", "pub "]
                    .iter()
                    .find_map(|p| next.strip_prefix(p))
                    .unwrap_or(next);
                let Some(name) = decl.strip_prefix("mod ").and_then(|r| r.strip_suffix(';')) else {
                    continue;
                };
                let stem = f.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                let parent = f.parent().unwrap();
                let dir = if matches!(stem, "mod" | "lib" | "main") {
                    parent.to_path_buf()
                } else {
                    parent.join(stem)
                };
                out.push(dir.join(format!("{name}.rs")));
                out.push(dir.join(name).join("mod.rs"));
            }
        }
        out
    }

    /// ⚠ The measure on the shapes a naive scanner gets wrong: a test module in mid-file, a `mod x;`
    /// declaration, a `'{'` char, a `;` inside `[u8; 2]`, a comment holding `}`, a raw string holding
    /// `"}`, and a `'static` lifetime that is not a char. Eight lines are code or doc.
    #[test]
    fn the_measure_skips_test_items_wherever_they_sit() {
        let src = r##"//! A module doc.

/// A doc.
fn a() {
    let s = "{";
    let c = '}';
}

#[cfg(test)]
mod early;

fn b() {}

#[cfg(test)]
mod tests {
    fn helper() -> char { '{' }
    const X: [u8; 2] = [1, 2];
    // a comment with a } brace
    fn raw() -> &'static str { r#"a"}"# }
}

#[cfg(test)]
const Y: [u8; 1] = [0];

fn c() {}
"##;
        assert_eq!(code_and_doc_lines(src), 8);
    }

    /// ⚠⚠ M10.7d's ceiling: no source file over a thousand lines of code and doc comments. Nine were
    /// over on 2026-09-30, each grown there one reasonable addition at a time, and were split by
    /// responsibility — this is what stops the tenth. Tests do not count: a file's test tail is
    /// excluded wherever it sits, and a file declared `#[cfg(test)] mod x;` is skipped whole.
    #[test]
    fn no_file_is_over_a_thousand_lines_of_code_and_doc() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let mut files = Vec::new();
        for c in std::fs::read_dir(crates).unwrap().flatten() {
            rust_files(&c.path().join("src"), &mut files);
        }
        let test_only = test_only_files(&files);
        // Positive controls first: a walk that found nothing, a measure that reads nothing, or a
        // test-file rule that recognises nothing would all pass the ceiling below.
        assert!(files.len() > 300, "only {} source files found", files.len());
        assert!(
            test_only
                .iter()
                .any(|f| f.ends_with("settings/compat_tests.rs")),
            "the test-only rule missed settings/compat_tests.rs"
        );
        let mut sizes: Vec<(usize, String)> = files
            .iter()
            .filter(|f| !test_only.contains(f))
            .map(|f| {
                let rel = f
                    .strip_prefix(crates)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                (
                    code_and_doc_lines(&std::fs::read_to_string(f).unwrap()),
                    rel,
                )
            })
            .collect();
        sizes.sort_unstable_by(|a, b| b.cmp(a));
        assert!(
            sizes[0].0 > 500,
            "the measure reads too little: largest {:?}",
            sizes[0]
        );
        let over: Vec<_> = sizes.iter().filter(|(n, _)| *n > CEILING).collect();
        assert!(
            over.is_empty(),
            "over {CEILING} lines of code and doc — split by responsibility (the M10.7d spec): {over:?}"
        );
    }
}

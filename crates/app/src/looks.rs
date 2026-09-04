//! Finding and loading the creative looks a user has installed.
//!
//! ⚠⚠ **Rustroom ships no `.cube` files.** The same reasoning as `camera_profile::store`:
//! redistributing someone else's look is a legal determination this project cannot make, and
//! discovering what the user already has sidesteps it entirely.
//!
//! ⚠ A **lib** module, not a bin one: everything here is testable without opening a window, and
//! the panel's job is only to show what this returns.

use std::path::{Path, PathBuf};

/// A `.cube` found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LookEntry {
    /// The file name, e.g. `"portra.cube"` — **exactly what goes into `Edits.look.file`**.
    ///
    /// ⚠ A name and not a path, so a preset carrying a look is shareable. Camera profiles
    /// already behave this way.
    pub file: String,
    /// What to show in the dropdown: the file stem.
    pub name: String,
}

/// Why a look could not be loaded.
///
/// ⚠⚠ **Every variant must be sayable in the panel.** A look that silently does nothing is
/// indistinguishable from a look that is not applied, which is the failure the spec's §9.2
/// forbids — so a missing or broken `.cube` renders without the look *and says so*.
#[derive(Debug)]
pub enum LookError {
    /// The name is not a plain file name — a separator or `..`. Refused before touching disk.
    UnsafeName,
    /// No such file in the looks directory.
    Missing,
    Unreadable(std::io::Error),
    Malformed(pipeline::lut::CubeError),
}

impl std::fmt::Display for LookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LookError::UnsafeName => write!(f, "not a look file name"),
            LookError::Missing => write!(f, "not in the looks folder"),
            LookError::Unreadable(e) => write!(f, "could not be read: {e}"),
            LookError::Malformed(e) => write!(f, "is not a usable .cube: {e}"),
        }
    }
}

/// The directory looks are read from. Path injected, like every other store here, so tests never
/// touch the real config directory.
pub struct LookStore {
    dir: PathBuf,
}

impl LookStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// `<platform config dir>/rustroom/looks`, beside `profiles/`, `presets/` and
    /// `settings.json`.
    ///
    /// ⚠ Not `~/.config` everywhere — `dirs::config_dir()` follows each platform's convention,
    /// which on macOS is `~/Library/Application Support/rustroom/`.
    pub fn user_default(config_dir: &Path) -> Self {
        Self::new(config_dir.join("rustroom").join("looks"))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Every `.cube` in the directory, sorted by name.
    ///
    /// ⚠ **Listing does not parse.** A 64³ cube is 786 KB and a directory may hold dozens; the
    /// dropdown only needs names. Parsing happens in [`load`](Self::load), when one is chosen.
    /// That also means a corrupt file still *appears* in the list and reports its error when
    /// picked, rather than vanishing with no explanation.
    pub fn list(&self) -> Vec<LookEntry> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            // ⚠ A missing directory is empty, not an error — the common case on a fresh install.
            return Vec::new();
        };
        let mut out: Vec<LookEntry> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("cube"))
            })
            .filter_map(|path| {
                let file = path.file_name()?.to_string_lossy().into_owned();
                let name = path.file_stem()?.to_string_lossy().into_owned();
                Some(LookEntry { file, name })
            })
            .collect();
        out.sort_by_key(|e| e.name.to_lowercase());
        out
    }

    /// Read and parse one look by file name.
    pub fn load(&self, file: &str) -> Result<pipeline::lut::Cube, LookError> {
        // ⚠⚠ **A file NAME, checked before it is joined.** `Edits.look.file` arrives from a
        // sidecar or a preset written on another machine, so it is untrusted text; `join` with
        // `"../../etc/passwd"` — or with an absolute path, which *replaces* the base entirely —
        // would read outside the looks folder. Rejecting anything that is not a single plain
        // component is the whole guard.
        let mut parts = Path::new(file).components();
        let one_plain_name = matches!(
            (parts.next(), parts.next()),
            (Some(std::path::Component::Normal(_)), None)
        );
        if !one_plain_name {
            return Err(LookError::UnsafeName);
        }
        let path = self.dir.join(file);
        let text = std::fs::read_to_string(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                LookError::Missing
            } else {
                LookError::Unreadable(e)
            }
        })?;
        pipeline::lut::Cube::parse(&text).map_err(LookError::Malformed)
    }
}

/// What the Rendering panel knows about looks: what is installed, and whether the one this photo
/// names actually loaded.
///
/// ⚠ **One struct rather than three fields on `RustroomApp`.** That literal is exhaustive on
/// purpose and contended by every track, so a single appended field is both less churn and less
/// merge conflict — and these three only ever change together.
///
/// ⚠⚠ Its whole reason to exist is `status`: a look that silently does nothing is
/// indistinguishable from a look that is not applied (spec §9.2), so the panel has to be able to
/// *say* what happened.
#[derive(Debug, Default, Clone)]
pub struct LooksState {
    /// Every `.cube` found in the looks directory, sorted.
    pub entries: Vec<LookEntry>,
    /// The file name `status` was computed for, so the check runs on change rather than per
    /// frame. ⚠ This body runs sixty times a second; re-reading a 786 KB table in it is the
    /// `apply_view_demosaic` mistake.
    resolved: Option<String>,
    /// `None` when the named look is fine (or there is none). `Some(msg)` is shown in the panel.
    pub status: Option<String>,
}

impl LooksState {
    /// A state holding exactly these entries, with nothing resolved yet.
    ///
    /// ⚠ A constructor because `resolved` is private — the panel's tests need to build one from
    /// a list without touching disk, and exposing the cache key so they could use `..Default`
    /// would let a caller desynchronise it from `status`.
    pub fn from_entries(entries: Vec<LookEntry>) -> Self {
        Self {
            entries,
            ..Default::default()
        }
    }

    /// Re-read the looks directory. Called when a session opens, not per frame.
    pub fn refresh(&mut self, store: &LookStore) {
        self.entries = store.list();
        // ⚠ Force a re-check: a file may have appeared or gone since the last resolve.
        self.resolved = None;
    }

    /// Bring `status` up to date for the look this photo names. Cheap to call every frame — it
    /// only touches disk when the name has changed.
    pub fn sync(&mut self, file: Option<&str>, store: &LookStore) {
        if self.resolved.as_deref() == file {
            return;
        }
        self.resolved = file.map(str::to_owned);
        self.status = match file {
            None => None,
            Some(f) => match store.load(f) {
                Ok(_) => None,
                // ⚠ Names the file, because the point of the message is that the user can go and
                // put it there. "A look could not be loaded" is not actionable.
                Err(e) => Some(format!("“{f}” {e} — rendering without it")),
            },
        };
    }
}

/// The warnings the Rendering section must show about this photo's look, in order.
///
/// ⚠⚠ **A pure function in the lib, not a branch inside the panel body.** The panel test harness
/// reads the accessibility tree, which carries interactive widgets and not plain labels — so a
/// warning drawn with `ui.label` is invisible to a test, and "the panel must say X" would be
/// unguarded. Deciding *what to say* here makes it testable; the panel only draws the result.
/// That is the `crop_tool` / `mask_tool` pattern, and §5's first question: can this be tested
/// without opening a window?
pub fn notes(edits: &edit_model::Edits, looks: &LooksState) -> Vec<String> {
    let mut out = Vec::new();
    if !edits.look.is_active() {
        // ⚠ No file, or strength 0: nothing runs, so there is nothing to warn about. Warning
        // anyway is how a warning stops being read.
        return out;
    }
    // ⚠⚠ The invisible-state guard. A `Linear` look means `DisplayRenderingStage` does not run,
    // because that LUT *is* the rendering — two renderings apparently both switched on with one
    // silently ignored is the failure this project keeps recording.
    if edits.look.space.is_the_rendering() && edits.rendering == edit_model::Rendering::Camera {
        out.push("This look is the rendering — camera rendering is not applied".to_owned());
    }
    // ⚠⚠ **Two sources, because they answer at different times.** "Not installed" is decided
    // from the list this frame, so a preset applied mid-session warns immediately; "installed
    // but corrupt" can only be known by reading the file, which `sync` does once when the photo
    // opens. Only `status` would leave a pasted look silent until the next switch away and back.
    let named = edits.look.file.as_deref();
    let installed = named.is_some_and(|f| looks.entries.iter().any(|e| e.file == f));
    if let Some(f) = named.filter(|_| !installed) {
        out.push(format!(
            "“{f}” is not in your looks folder — rendering without it"
        ));
    } else if let Some(msg) = &looks.status {
        out.push(msg.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity_text(n: usize) -> String {
        let mut s = format!("LUT_3D_SIZE {n}\n");
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let f = |i: usize| i as f32 / (n - 1) as f32;
                    s.push_str(&format!("{} {} {}\n", f(r), f(g), f(b)));
                }
            }
        }
        s
    }

    /// A unique temp directory for one test, removed on drop. Keeps tests off the real config
    /// directory without adding a `tempfile` dependency — the `presets.rs` precedent.
    ///
    /// ⚠ The tag must be unique per test: these run in parallel threads in one process, and two
    /// sharing a directory produce a failure that looks like a bug in the code under test.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let p =
                std::env::temp_dir().join(format!("rustroom_looks_{tag}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn store_with(tag: &str, files: &[(&str, &str)]) -> (TempDir, LookStore) {
        let dir = TempDir::new(tag);
        let looks = dir.0.join("rustroom").join("looks");
        std::fs::create_dir_all(&looks).expect("mkdir");
        for (name, body) in files {
            std::fs::write(looks.join(name), body).expect("write");
        }
        let store = LookStore::user_default(&dir.0);
        (dir, store)
    }

    /// ⚠ `<config>/rustroom/looks`, beside `profiles/` — one convention, not a second one.
    #[test]
    fn the_looks_directory_sits_beside_the_profiles_directory() {
        let base = Path::new("/tmp/cfg");
        assert_eq!(
            LookStore::user_default(base).dir(),
            Path::new("/tmp/cfg/rustroom/looks")
        );
        assert_eq!(
            camera_profile::store::ProfileStore::user_default(base)
                .dir()
                .parent(),
            LookStore::user_default(base).dir().parent(),
            "the two stores have drifted apart"
        );
    }

    /// ⚠ A missing directory is empty rather than an error — the fresh-install case.
    #[test]
    fn a_missing_looks_directory_lists_nothing() {
        let dir = TempDir::new("missing");
        assert!(LookStore::user_default(&dir.0).list().is_empty());
    }

    #[test]
    fn only_cube_files_are_listed_and_they_are_sorted() {
        let (_d, store) = store_with(
            "listing",
            &[
                ("Velvia.cube", &identity_text(2)),
                ("astia.CUBE", &identity_text(2)),
                ("notes.txt", "hello"),
                ("portra.cube", &identity_text(2)),
            ],
        );
        let names: Vec<_> = store.list().into_iter().map(|e| e.name).collect();
        // ⚠ Case-insensitive sort, and `.CUBE` counts: a downloaded look's extension case is
        // not the user's doing.
        assert_eq!(names, vec!["astia", "portra", "Velvia"]);
    }

    /// ⚠ The dropdown shows the stem; `Edits.look.file` stores the whole file name. Storing the
    /// stem would make the look unloadable on a `.CUBE`.
    #[test]
    fn the_entry_stores_the_file_name_and_shows_the_stem() {
        let (_d, store) = store_with("stem", &[("Kodak 2383.cube", &identity_text(2))]);
        let e = &store.list()[0];
        assert_eq!(e.file, "Kodak 2383.cube");
        assert_eq!(e.name, "Kodak 2383");
    }

    /// ⚠⚠ **A corrupt `.cube` still LISTS and reports its error when picked.** Dropping it from
    /// the list would leave the user with a file they can see in Finder and not in Rustroom,
    /// with nothing said — the silent failure §9.2 forbids.
    #[test]
    fn a_corrupt_cube_lists_but_refuses_to_load() {
        let (_d, store) = store_with("corrupt", &[("broken.cube", "LUT_3D_SIZE 3\n0 0 0\n")]);
        assert_eq!(
            store.list().len(),
            1,
            "a broken look vanished from the list"
        );
        assert!(matches!(
            store.load("broken.cube"),
            Err(LookError::Malformed(_))
        ));
    }

    #[test]
    fn a_look_that_is_not_there_is_missing_rather_than_a_panic() {
        let (_d, store) = store_with("absent", &[]);
        assert!(matches!(store.load("gone.cube"), Err(LookError::Missing)));
    }

    /// ⚠⚠ **`Edits.look.file` is untrusted text** — it arrives from a sidecar or a preset written
    /// elsewhere. A `..` traversal, a separator, or an absolute path (which `join` does not
    /// append but *replaces* with) must be refused before touching disk.
    #[test]
    fn a_name_that_escapes_the_looks_folder_is_refused() {
        let (_d, store) = store_with("escape", &[("ok.cube", &identity_text(2))]);
        for bad in [
            "../ok.cube",
            "sub/ok.cube",
            "/etc/passwd",
            "..",
            "",
            "./ok.cube",
        ] {
            assert!(
                matches!(store.load(bad), Err(LookError::UnsafeName)),
                "{bad:?} was not refused"
            );
        }
        // The control: the same file by its plain name loads.
        assert!(store.load("ok.cube").is_ok());
    }

    /// ⚠⚠ **A missing look SAYS SO.** The silent-failure case the spec's §9.2 forbids: a preset
    /// carrying a look is portable only where the `.cube` is, and the user has to be told which
    /// file to go and find.
    #[test]
    fn a_missing_look_produces_a_status_naming_the_file() {
        let (_d, store) = store_with("status", &[("here.cube", &identity_text(2))]);
        let mut st = LooksState::default();
        st.refresh(&store);
        assert_eq!(st.entries.len(), 1);

        st.sync(Some("here.cube"), &store);
        assert_eq!(st.status, None, "a look that is present must not warn");

        st.sync(Some("gone.cube"), &store);
        let msg = st.status.clone().expect("a missing look said nothing");
        assert!(
            msg.contains("gone.cube"),
            "the message does not name it: {msg}"
        );

        st.sync(None, &store);
        assert_eq!(st.status, None, "no look, no warning");
    }

    /// ⚠ Present but corrupt is a *different* message from missing, and both must appear — a
    /// truncated download and a file you never copied are different problems.
    #[test]
    fn a_corrupt_look_produces_a_different_status_from_a_missing_one() {
        let (_d, store) = store_with("status2", &[("bad.cube", "LUT_3D_SIZE 3\n0 0 0\n")]);
        let mut st = LooksState::default();
        st.sync(Some("bad.cube"), &store);
        let corrupt = st.status.clone().expect("a corrupt look said nothing");
        st.sync(Some("absent.cube"), &store);
        let missing = st.status.clone().expect("a missing look said nothing");
        assert_ne!(corrupt, missing);
        assert!(corrupt.contains("cube"), "{corrupt}");
    }

    /// ⚠ `sync` only touches disk when the name changes — it is called from a body that runs
    /// sixty times a second.
    #[test]
    fn sync_does_not_re_read_an_unchanged_name() {
        let (d, store) = store_with("cache", &[("x.cube", &identity_text(2))]);
        let mut st = LooksState::default();
        st.sync(Some("x.cube"), &store);
        assert_eq!(st.status, None);
        // Delete it behind `sync`'s back. An unchanged name must not notice, which is exactly
        // what proves the disk was not touched again.
        std::fs::remove_file(d.0.join("rustroom").join("looks").join("x.cube")).unwrap();
        st.sync(Some("x.cube"), &store);
        assert_eq!(
            st.status, None,
            "sync re-read the file it had already resolved"
        );
    }

    const REPLACES: &str = "This look is the rendering — camera rendering is not applied";

    fn installed() -> LooksState {
        LooksState::from_entries(vec![LookEntry {
            file: "portra.cube".into(),
            name: "portra".into(),
        }])
    }

    fn a_look(file: &str, space: edit_model::LutSpace) -> edit_model::Edits {
        edit_model::Edits {
            rendering: edit_model::Rendering::Camera,
            look: edit_model::Look {
                file: Some(file.into()),
                space,
                strength: 100.0,
            },
            ..Default::default()
        }
    }

    /// ⚠⚠ **The invisible-state guard.** A `Linear` look means `DisplayRenderingStage` does not
    /// run, so the panel must say the camera rendering is not applied.
    ///
    /// ⚠ All three directions. A note pushed unconditionally would pass the first assertion and
    /// be a permanent scare on every display-referred look; one that ignored `edits.rendering`
    /// would warn about a rendering that was already off, which is how a warning stops being read.
    #[test]
    fn a_linear_look_says_the_camera_rendering_is_not_applied() {
        let looks = installed();
        let linear = a_look("portra.cube", edit_model::LutSpace::Linear);
        assert!(notes(&linear, &looks).iter().any(|n| n == REPLACES));

        let display = a_look("portra.cube", edit_model::LutSpace::SrgbDisplay);
        assert!(
            !notes(&display, &looks).iter().any(|n| n == REPLACES),
            "a display-referred look wrongly claimed to replace the rendering"
        );

        let off = edit_model::Edits {
            rendering: edit_model::Rendering::None,
            ..linear.clone()
        };
        assert!(
            !notes(&off, &looks).iter().any(|n| n == REPLACES),
            "warned about a rendering that was not on"
        );
    }

    /// ⚠ A preset carrying a look references a file by NAME, so it is portable only where the
    /// `.cube` is present. A missing one renders without the look **and says so** (spec §9.2).
    #[test]
    fn a_look_this_machine_does_not_have_says_so() {
        let looks = installed();
        let missing = a_look("velvia.cube", edit_model::LutSpace::SrgbDisplay);
        let n = notes(&missing, &looks);
        assert!(
            n.iter()
                .any(|m| m.contains("velvia.cube") && m.contains("not in your looks")),
            "a missing look said nothing: {n:?}"
        );

        // The control: an installed look draws no such note.
        let present = a_look("portra.cube", edit_model::LutSpace::SrgbDisplay);
        assert!(
            !notes(&present, &looks)
                .iter()
                .any(|m| m.contains("not in your looks")),
            "an installed look was reported missing"
        );
    }

    /// ⚠ An inactive look says nothing at all — no file, or strength 0, means nothing runs.
    #[test]
    fn an_inactive_look_produces_no_notes() {
        let looks = installed();
        assert!(notes(&edit_model::Edits::default(), &looks).is_empty());

        let mut zero = a_look("velvia.cube", edit_model::LutSpace::Linear);
        zero.look.strength = 0.0;
        assert!(
            notes(&zero, &looks).is_empty(),
            "a look at zero strength warned about things that do not happen"
        );
    }

    /// ⚠ Installed-but-corrupt is a different note from not-installed, and reaches the panel
    /// through `status` because only reading the file can tell.
    #[test]
    fn a_corrupt_installed_look_reports_the_status_message() {
        let (_d, store) = store_with("notes", &[("bad.cube", "LUT_3D_SIZE 3\n0 0 0\n")]);
        let mut looks = LooksState::default();
        looks.refresh(&store);
        looks.sync(Some("bad.cube"), &store);
        let n = notes(
            &a_look("bad.cube", edit_model::LutSpace::SrgbDisplay),
            &looks,
        );
        assert_eq!(n.len(), 1, "{n:?}");
        assert!(
            n[0].contains("bad.cube") && !n[0].contains("not in your looks"),
            "{n:?}"
        );
    }

    #[test]
    fn a_good_cube_loads_and_is_the_identity() {
        let (_d, store) = store_with("good", &[("id.cube", &identity_text(5))]);
        let cube = store.load("id.cube").expect("loads");
        assert_eq!(cube.lookup([0.25, 0.5, 0.75]), [0.25, 0.5, 0.75]);
    }
}

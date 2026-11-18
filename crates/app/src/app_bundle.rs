//! The libraries' run-time folders inside the Mac app (v0.1, piece 3b). `Rustroom.app` carries libgphoto2's camera and
//! port drivers and an empty plugin folder for libheif (`packaging/macos/build.sh`); both libraries otherwise look in
//! the folders Homebrew compiled into them, which a user's Mac may not have — or may have with other versions. So at
//! startup, inside a bundle, Rustroom names its own folders through the variables the libraries read.
//!
//! ⚠ A lib module that is a no-op off macOS and outside a bundle, so its one call in `main` needs no `cfg` (the lesson
//! of the macOS-only modules, `AGENTS.md`; `console`'s pattern).

use std::path::{Path, PathBuf};

/// The folders a bundle carries for the libraries to read at run time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeDirs {
    /// libgphoto2's camera drivers — `CAMLIBS`.
    pub camlibs: PathBuf,
    /// libgphoto2's port drivers (USB, PTP/IP, …) — `IOLIBS`.
    pub iolibs: PathBuf,
    /// An empty folder, so libheif loads no plugin built against another libheif — `LIBHEIF_PLUGIN_PATH`.
    pub heif_plugins: PathBuf,
}

/// The bundle's folders when `exe` is a bundle's executable — `<name>.app/Contents/MacOS/<exe>` — else `None`.
/// Paths only: nothing is read from the disk.
pub fn runtime_dirs(exe: &Path) -> Option<RuntimeDirs> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    let named = |p: &Path, name: &str| p.file_name().is_some_and(|n| n == name);
    let is_app = app
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(".app"));
    if !named(macos, "MacOS") || !named(contents, "Contents") || !is_app {
        return None;
    }
    let plugins = contents.join("PlugIns");
    Some(RuntimeDirs {
        camlibs: plugins.join("libgphoto2").join("camlibs"),
        iolibs: plugins.join("libgphoto2").join("iolibs"),
        heif_plugins: plugins.join("libheif"),
    })
}

/// The variables to set, in order, leaving out every one `is_set` says is already set — a value the user gave
/// (`launchctl setenv`, a terminal) always wins.
pub fn to_set(dirs: &RuntimeDirs, is_set: impl Fn(&str) -> bool) -> Vec<(&'static str, PathBuf)> {
    [
        ("CAMLIBS", &dirs.camlibs),
        ("IOLIBS", &dirs.iolibs),
        ("LIBHEIF_PLUGIN_PATH", &dirs.heif_plugins),
    ]
    .into_iter()
    .filter(|(k, _)| !is_set(k))
    .map(|(k, p)| (k, p.clone()))
    .collect()
}

/// The bundle's folders for the running executable `exe`, as `std::env::current_exe` gives it — through a symlink
/// to the file it points at, as dyld resolves it (a path that cannot be resolved is read as it is). A translocated
/// app's path is a real one and stays as it is.
pub fn dirs_for_running(exe: &Path) -> Option<RuntimeDirs> {
    let real = std::fs::canonicalize(exe).unwrap_or_else(|_| exe.to_path_buf());
    runtime_dirs(&real)
}

/// On macOS, inside a bundle, points libgphoto2 and libheif at the bundle's own folders; anything else does nothing.
///
/// ⚠ **The first line of `main`, before any thread starts:** `std::env::set_var` is not thread-safe — another thread
/// reading the environment meanwhile (libgphoto2's camera search, a decode's libheif) is undefined behaviour.
pub fn point_libraries_inside() {
    #[cfg(target_os = "macos")]
    {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let Some(dirs) = dirs_for_running(&exe) else {
            return;
        };
        for (name, dir) in to_set(&dirs, |k| std::env::var_os(k).is_some()) {
            std::env::set_var(name, dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folders(app: &Path) -> RuntimeDirs {
        let plugins = app.join("Contents").join("PlugIns");
        RuntimeDirs {
            camlibs: plugins.join("libgphoto2").join("camlibs"),
            iolibs: plugins.join("libgphoto2").join("iolibs"),
            heif_plugins: plugins.join("libheif"),
        }
    }

    fn exe_in(app: &Path) -> PathBuf {
        app.join("Contents").join("MacOS").join("Rustroom")
    }

    #[test]
    fn a_bundled_executable_finds_its_folders_inside_the_bundle() {
        let installed = Path::new("/Applications").join("Rustroom.app");
        assert_eq!(runtime_dirs(&exe_in(&installed)), Some(folders(&installed)));
        // A space in the path — a folder of the user's, or a mounted "Rustroom 0.1.0" volume.
        let spaced = Path::new("/Users/x").join("My Apps").join("Rustroom.app");
        assert_eq!(runtime_dirs(&exe_in(&spaced)), Some(folders(&spaced)));
    }

    #[test]
    fn an_executable_outside_a_bundle_has_none() {
        let root = Path::new("/x");
        let cases = [
            root.join("target").join("release").join("Rustroom"),
            root.join("Contents").join("MacOS").join("Rustroom"),
            root.join("Rustroom.app").join("MacOS").join("Rustroom"),
            root.join("Rustroom.app")
                .join("Contents")
                .join("Resources")
                .join("Rustroom"),
        ];
        for exe in cases {
            assert_eq!(runtime_dirs(&exe), None, "{exe:?}");
        }
    }

    /// ⚠ macOS hands `current_exe` "a path" to the executable — a symlink to it stays a symlink — while dyld finds
    /// `Frameworks` from the real one: a link in `~/bin` would run the app with Homebrew's drivers (the review of
    /// piece 3b). The folders follow the file the link points at.
    #[cfg(unix)]
    #[test]
    fn a_symlink_to_the_executable_still_finds_the_bundle() {
        let d = test_dir::TempDir::new("app-bundle-link");
        let app = d.join("Rustroom.app");
        let macos = app.join("Contents").join("MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        std::fs::write(macos.join("Rustroom"), b"").unwrap();
        std::fs::create_dir_all(d.join("bin")).unwrap();
        let link = d.join("bin").join("rustroom");
        std::os::unix::fs::symlink(macos.join("Rustroom"), &link).unwrap();
        // The premise: the link's own path names no bundle.
        assert_eq!(runtime_dirs(&link), None);
        let real = std::fs::canonicalize(&app).unwrap();
        assert_eq!(dirs_for_running(&link), Some(folders(&real)));
        // And an executable that does not exist is still answered from its path.
        let gone = Path::new("/Applications").join("Rustroom.app");
        assert_eq!(dirs_for_running(&exe_in(&gone)), Some(folders(&gone)));
    }

    #[test]
    fn a_variable_already_set_is_never_replaced() {
        let dirs = folders(&Path::new("/Applications").join("Rustroom.app"));
        let names =
            |set: &[(&'static str, PathBuf)]| set.iter().map(|(k, _)| *k).collect::<Vec<_>>();
        // The premise: with nothing set, all three are named, in order.
        let all = to_set(&dirs, |_| false);
        assert_eq!(names(&all), ["CAMLIBS", "IOLIBS", "LIBHEIF_PLUGIN_PATH"]);
        let some = to_set(&dirs, |k| k == "IOLIBS");
        assert_eq!(
            some,
            vec![
                ("CAMLIBS", dirs.camlibs.clone()),
                ("LIBHEIF_PLUGIN_PATH", dirs.heif_plugins.clone()),
            ]
        );
        assert!(to_set(&dirs, |_| true).is_empty());
    }
}

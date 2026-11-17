//! Windows' console (v0.1): the app is a GUI program there (`windows_subsystem` in `main.rs`), so a double-click
//! opens no black console window beside it — and started from a terminal it borrows that terminal's console, so
//! `cargo run` still shows the `RUSTROOM_DEBUG_*` output.
//!
//! ⚠ A lib module that is a no-op off Windows, so its one call in `main` needs no `cfg` (the lesson of the
//! macOS-only modules, `AGENTS.md`).

/// Attach to the console of the process that started this one, when it has one. A redirected output
/// (`Rustroom.exe > log`) keeps its file: a handle already set is not replaced.
pub fn attach_to_parent() {
    #[cfg(windows)]
    {
        const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
        #[link(name = "kernel32")]
        extern "system" {
            fn AttachConsole(process_id: u32) -> i32;
        }
        // SAFETY: takes no pointers, and fails harmlessly (returns 0) when the parent has no console.
        unsafe {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

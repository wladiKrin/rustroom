# `rsraw-sys` 0.1.1, vendored for Windows

`rsraw-sys` compiles LibRaw 0.21.3 into Rustroom. The published 0.1.1 refuses to build with MSVC; LibRaw itself
builds with MSVC given one define. This directory is that release, kept **byte for byte** (`vendor/** -text` in the root
`.gitattributes`), plus one patch to `build.rs` — `git log -p -- vendor/rsraw-sys` shows it on its own. It needs
no `LIBCLANG_PATH` on Windows: bindgen finds LLVM's default install by itself (checked 2026-11-17).

1. **No MSVC refusal.** The build script's `panic!("MSVC is not supported")` is gone.
2. **`LIBRAW_NODLL` on MSVC.** `libraw.h` turns on its DLL declarations for any `_MSC_VER` compiler (line 61), so
   without it every function is `dllimport` and its definition fails (C2491). This build is a static library.
3. **`-pthread` only for compilers that know it.** MSVC does not, and LibRaw needs no flag there.
4. **Bindgen blocklists `memcmp`, `memcpy`, `memmove`, `memset` and `strlen`.** `libraw.h` pulls them in through the C
   headers on every system; on Windows bindgen writes their sizes as `u64` instead of `usize`, and rustc warns that the
   crate redefines the C runtime's own functions with the wrong signature. The `rsraw` wrapper uses none of them.

On macOS and Linux LibRaw compiles exactly as published; the only difference there is that the bindings no longer
declare those five unused functions (the crate's own pre-generated, glibc `src/bindings.rs` has them at lines 631–801).

**Taking a new release:** copy it over this directory (minus `.cargo-ok` and `.cargo_vcs_info.json`), commit that
unchanged, then re-apply the patch above if the release still needs it — or delete this directory and the root
`Cargo.toml`'s `[patch.crates-io]` once an upstream release builds with MSVC.

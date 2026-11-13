# Windows: parked

**Status: parked, unmaintained. Do not extend this code.**

This round of work is a macOS refinement round (see `docs/PLAN.md`). To keep
the repo focused and the macOS build free of Windows references, the Windows
code was moved here with `git mv` (history follows the files). Nothing was
deleted. Windows is meant to come back later. Until then this code is not
built, tested, or kept in step with macOS changes, so expect it to need work
when it is restored.

## What is parked and where it came from

| Parked path                                                   | Original path                         | What it is                                                      |
|---------------------------------------------------------------|---------------------------------------|-----------------------------------------------------------------|
| `platforms-parked/windows/service-tool/`                      | `service-tool/`                       | Windows privileged service (Rust, named pipe, Win32 IP Helper)  |
| `platforms-parked/windows/installer-iss/`                     | `installer-iss/`                      | Inno Setup 6 installer script (`installer.iss`)                 |
| `platforms-parked/windows/src-tauri/src/platform/windows.rs`  | `src-tauri/src/platform/windows.rs`   | Windows platform module of the Tauri app                        |
| `platforms-parked/windows/src-tauri/tauri.windows.conf.json`  | `src-tauri/tauri.windows.conf.json`   | Tauri config overlay for Windows (custom titlebar, no devtools) |

Not moved: the Windows-specific JS/CSS in `src/` (custom titlebar etc.) and
the `[target.'cfg(target_os = "windows")'.dependencies]` section in
`src-tauri/Cargo.toml` (it holds only comments). They still sit in their
normal places.

`installer.iss` uses paths relative to its own folder (`..\src-tauri\...`,
`..\service-tool\...`, `..\installer-build`). They only resolve from the
original location, so restore before building anything here.

## How to restore

Run from the repo root.

1. Move the four items back (keeps history):
   ```bash
   git mv platforms-parked/windows/service-tool service-tool
   git mv platforms-parked/windows/installer-iss installer-iss
   git mv platforms-parked/windows/src-tauri/src/platform/windows.rs src-tauri/src/platform/windows.rs
   git mv platforms-parked/windows/src-tauri/tauri.windows.conf.json src-tauri/tauri.windows.conf.json
   ```
2. Re-add the Windows module to `src-tauri/src/platform/mod.rs`, below the
   macOS lines, and remove the "Windows is parked" comment:
   ```rust
   #[cfg(target_os = "windows")]
   mod windows;
   #[cfg(target_os = "windows")]
   pub use windows::*;
   ```
3. Re-add the two Windows entries to `scripts/bump-version.sh`, and put them
   back in the header's file list (and count):
   ```bash
   # with the other Cargo.toml entries (REPLACEMENT="version = ...")
   edit_file "service-tool/Cargo.toml"    'version = "[0-9]+\.[0-9]+\.[0-9]+"' "Cargo (service-tool)"

   # installer.iss — `#define AppVersion "X.Y.Z"`
   REPLACEMENT="#define AppVersion \"${NEW_VERSION}\""
   edit_file "installer-iss/installer.iss" '#define AppVersion "[0-9]+\.[0-9]+\.[0-9]+"' "Inno Setup"
   ```
4. In `.gitignore`, replace `platforms-parked/windows/service-tool/target/`
   with `service-tool/target/`.
5. Update the paths that point here: the comment in the
   `[target.'cfg(target_os = "windows")'.dependencies]` section of
   `src-tauri/Cargo.toml`, and the Windows build steps in the root
   `README.md` (drop the "parked" note).
6. Remove this folder once it is empty.
7. Check it builds on a Windows machine, then bring the code up to date with
   the macOS changes made while it was parked (the helper protocol, input
   validation, change journal and rescue tooling were all reworked).

`.githooks/pre-commit` already lists `service-tool` and skips it while its
`Cargo.toml` is missing, so it needs no change.

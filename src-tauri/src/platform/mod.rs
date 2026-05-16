// Platform dispatch. At compile time, Rust compiles only the module that
// matches the target OS. The rest of the codebase calls into this module
// without knowing which platform it's on.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::*;
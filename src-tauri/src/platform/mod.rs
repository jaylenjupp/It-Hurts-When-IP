// Platform dispatch. At compile time, Rust compiles only the module that
// matches the target OS. The rest of the codebase calls into this module
// without knowing which platform it's on.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

// Windows is parked this round: its module lives in platforms-parked/windows/
// (see the README there for how to restore it).

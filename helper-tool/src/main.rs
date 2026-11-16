// It Hurts When IP privileged helper (root LaunchDaemon).
//
// Listens on a Unix socket that only root and the admin group can use
// (0660 root:admin), reads one JSON request per connection, and runs only the
// allow-listed, validated networksetup commands in netsetup.rs (argv, never a
// shell). See protocol.rs for the wire format.
//
// Release builds use fixed paths and never read IHWIP_* environment
// variables. Debug builds are for tests on the build host only: they refuse
// to run as root, refuse the real socket and the real networksetup, and need
// IHWIP_HELPER_SOCKET, IHWIP_HELPER_JOURNAL and IHWIP_NETWORKSETUP.

mod error;
mod netsetup;
mod protocol;
mod runner;
mod validate;

use protocol::{Ctx, ReadError};
use runner::SystemRunner;
use std::fs;
use std::io::{self, Write as _};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

const RELEASE_SOCKET: &str = "/var/run/com.ithurtswhenip.helper.sock";
/// Reserved for the change journal (package 2.2).
#[cfg(not(debug_assertions))]
const RELEASE_JOURNAL: &str = "/Library/Application Support/com.ithurtswhenip.helper/journal.json";

/// rw for root and the admin group, nothing for anyone else. Never wider.
const SOCKET_MODE: u32 = 0o660;
/// Permission bits of st_mode, including setuid/setgid/sticky.
const MODE_BITS: u32 = 0xFFF;
#[cfg(not(debug_assertions))]
const ROOT_UID: u32 = 0;
/// The macOS `admin` group (P2-D2).
#[cfg(not(debug_assertions))]
const ADMIN_GID: u32 = 80;

/// Read and write timeout on each client connection.
const IO_TIMEOUT: Duration = Duration::from_secs(5);

struct Config {
    socket: PathBuf,
    /// Not used until the change journal lands (package 2.2).
    journal: PathBuf,
    networksetup: String,
}

fn main() -> ExitCode {
    let config = match load_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ithurtswhenip-helper: not starting: {}", e);
            return ExitCode::FAILURE;
        }
    };
    let listener = match open_socket(&config.socket) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("ithurtswhenip-helper: not starting: {}", e);
            return ExitCode::FAILURE;
        }
    };
    println!(
        "Helper {} (protocol {}, git {}) listening on {}; networksetup {}; journal {}",
        protocol::HELPER_VERSION,
        protocol::PROTOCOL_VERSION,
        protocol::GIT_SHA,
        config.socket.display(),
        config.networksetup,
        config.journal.display(),
    );
    serve(&listener, &config.networksetup);
    ExitCode::SUCCESS
}

// --- Configuration ---

/// Release: fixed paths only. No environment variable is read.
#[cfg(not(debug_assertions))]
fn load_config() -> Result<Config, String> {
    Ok(Config {
        socket: PathBuf::from(RELEASE_SOCKET),
        journal: PathBuf::from(RELEASE_JOURNAL),
        networksetup: netsetup::SYSTEM_NETWORKSETUP.to_string(),
    })
}

/// Debug: never as root, and only with all three paths from the environment.
#[cfg(debug_assertions)]
fn load_config() -> Result<Config, String> {
    if running_as_root()? {
        return Err(
            "this is a debug build running as root. Debug builds never run as root; \
                    only the release helper installed by the .pkg does."
                .to_string(),
        );
    }
    config_from_env(|name| std::env::var_os(name))
}

/// No libc: create a probe file in the temp dir and read its owner. A new
/// file is owned by the effective user id.
#[cfg(debug_assertions)]
fn running_as_root() -> Result<bool, String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let probe = std::env::temp_dir().join(format!(
        "ihwip-helper-uid-probe-{}-{}",
        std::process::id(),
        nanos
    ));
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .map_err(|e| {
            format!(
                "could not create {} to check the user id: {}",
                probe.display(),
                e
            )
        })?;
    let uid = file.metadata().map(|m| m.uid());
    drop(file);
    let _ = fs::remove_file(&probe);
    let uid = uid.map_err(|e| format!("could not read the owner of {}: {}", probe.display(), e))?;
    Ok(uid == 0)
}

#[cfg(debug_assertions)]
fn config_from_env(get: impl Fn(&str) -> Option<std::ffi::OsString>) -> Result<Config, String> {
    let socket = absolute_env(&get, "IHWIP_HELPER_SOCKET")?;
    let journal = absolute_env(&get, "IHWIP_HELPER_JOURNAL")?;
    let networksetup = absolute_env(&get, "IHWIP_NETWORKSETUP")?;
    check_debug_socket(&socket)?;
    check_debug_networksetup(&networksetup)?;
    let networksetup = networksetup
        .to_str()
        .ok_or("IHWIP_NETWORKSETUP is not valid UTF-8")?
        .to_string();
    Ok(Config {
        socket,
        journal,
        networksetup,
    })
}

#[cfg(debug_assertions)]
fn absolute_env(
    get: &impl Fn(&str) -> Option<std::ffi::OsString>,
    name: &str,
) -> Result<PathBuf, String> {
    let value = get(name).filter(|v| !v.is_empty()).ok_or_else(|| {
        format!(
            "{} is not set. A debug build needs IHWIP_HELPER_SOCKET, IHWIP_HELPER_JOURNAL \
             and IHWIP_NETWORKSETUP, all absolute paths.",
            name
        )
    })?;
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(format!("{} must be an absolute path, got {:?}", name, path));
    }
    Ok(path)
}

/// The build host runs the real root helper: a debug build must never touch
/// its socket, not even by name or directory.
#[cfg(debug_assertions)]
fn check_debug_socket(socket: &Path) -> Result<(), String> {
    let release = Path::new(RELEASE_SOCKET);
    let canonical_parent = |p: &Path| p.parent().and_then(|d| fs::canonicalize(d).ok());
    let same_dir = match (canonical_parent(socket), canonical_parent(release)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    };
    if socket == release || socket.file_name() == release.file_name() || same_dir {
        return Err(format!(
            "IHWIP_HELPER_SOCKET {:?} is (or sits next to) the real helper socket {}. \
             Use a temp dir.",
            socket, RELEASE_SOCKET
        ));
    }
    Ok(())
}

/// Admin users can sometimes run real networksetup writes without root, so a
/// debug helper must only ever run the fake.
#[cfg(debug_assertions)]
fn check_debug_networksetup(path: &Path) -> Result<(), String> {
    let system = Path::new(netsetup::SYSTEM_NETWORKSETUP);
    let same_file = match (fs::canonicalize(path), fs::canonicalize(system)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    if path == system || path.file_name() == system.file_name() || same_file {
        return Err(format!(
            "IHWIP_NETWORKSETUP {:?} is the real networksetup. Debug builds only run \
             tools/fake-networksetup/fake-networksetup.",
            path
        ));
    }
    Ok(())
}

// --- Socket ---

/// Remove a stale socket, bind, then lock the socket down to 0660 root:admin
/// (debug: 0660, owned by the current user). If that fails the socket is
/// removed and the helper doesn't start: it never serves a wider socket.
fn open_socket(path: &Path) -> Result<UnixListener, String> {
    remove_stale_socket(path)?;
    // Under the LaunchDaemon's Umask 077 (see the plist) bind() creates the
    // socket owner-only, so it is never wider than 0660 at any moment.
    let listener = UnixListener::bind(path)
        .map_err(|e| format!("could not bind the socket {}: {}", path.display(), e))?;
    if let Err(e) = secure_socket(path) {
        drop(listener);
        let _ = fs::remove_file(path);
        return Err(format!("{} (socket removed)", e));
    }
    Ok(listener)
}

/// Only an old socket is removed. Anything else at the path is left alone
/// and reported, because with KeepAlive the helper will keep retrying and
/// this message in the error log is the only clue.
fn remove_stale_socket(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("could not check {}: {}", path.display(), e)),
        Ok(meta) if meta.file_type().is_socket() => fs::remove_file(path).map_err(|e| {
            format!(
                "could not remove the stale socket {}: {}",
                path.display(),
                e
            )
        }),
        Ok(meta) => Err(format!(
            "found a {} at {} (owner uid {}, mode {:o}) where the helper socket goes. \
             It is not a socket, so the helper won't remove it. Move it away by hand \
             and the helper will start.",
            kind_name(&meta),
            path.display(),
            meta.uid(),
            meta.mode() & MODE_BITS
        )),
    }
}

fn kind_name(meta: &fs::Metadata) -> &'static str {
    let t = meta.file_type();
    if t.is_symlink() {
        "symbolic link"
    } else if t.is_dir() {
        "directory"
    } else if t.is_file() {
        "regular file"
    } else if t.is_fifo() {
        "named pipe"
    } else if t.is_block_device() || t.is_char_device() {
        "device file"
    } else {
        "special file"
    }
}

/// Release: chown root:admin first (so the group is right before any group
/// bit is set), then chmod 0660, then read both back. Debug: chmod only.
fn secure_socket(path: &Path) -> Result<(), String> {
    #[cfg(not(debug_assertions))]
    std::os::unix::fs::chown(path, Some(ROOT_UID), Some(ADMIN_GID)).map_err(|e| {
        format!(
            "could not set the owner of {} to root:admin: {}",
            path.display(),
            e
        )
    })?;
    fs::set_permissions(path, fs::Permissions::from_mode(SOCKET_MODE)).map_err(|e| {
        format!(
            "could not set the mode of {} to {:o}: {}",
            path.display(),
            SOCKET_MODE,
            e
        )
    })?;

    let meta = fs::symlink_metadata(path)
        .map_err(|e| format!("could not read back {}: {}", path.display(), e))?;
    let mode = meta.mode() & MODE_BITS;
    if mode != SOCKET_MODE {
        return Err(format!(
            "{} has mode {:o} after chmod, expected {:o}",
            path.display(),
            mode,
            SOCKET_MODE
        ));
    }
    #[cfg(not(debug_assertions))]
    if meta.uid() != ROOT_UID || meta.gid() != ADMIN_GID {
        return Err(format!(
            "{} is owned by {}:{} after chown, expected {}:{}",
            path.display(),
            meta.uid(),
            meta.gid(),
            ROOT_UID,
            ADMIN_GID
        ));
    }
    Ok(())
}

// --- Serving ---

fn serve(listener: &UnixListener, networksetup: &str) {
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let networksetup = networksetup.to_string();
                std::thread::spawn(move || handle_client(stream, &networksetup));
            }
            Err(e) => eprintln!("connection error: {}", e),
        }
    }
}

/// One request per connection, as in v1.13.1.
fn handle_client(stream: UnixStream, networksetup: &str) {
    if let Err(e) = stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .and_then(|_| stream.set_write_timeout(Some(IO_TIMEOUT)))
    {
        eprintln!("could not set connection timeouts, dropping it: {}", e);
        return;
    }
    let reply = match protocol::read_request(&stream) {
        Ok(line) => {
            let ctx = Ctx {
                runner: &SystemRunner,
                networksetup,
            };
            protocol::handle_line(&line, &ctx)
        }
        Err(ReadError::Io(e)) => {
            eprintln!("could not read request: {}", e);
            return;
        }
        Err(e) => {
            eprintln!("invalid request: {:?}", e);
            protocol::invalid_request()
        }
    };
    let mut writer = &stream;
    if let Err(e) = writer.write_all(format!("{}\n", reply).as_bytes()) {
        eprintln!("could not send reply: {}", e);
    }
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static N: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "ihwip-h-unit-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const FAKE: &str = "/repo/tools/fake-networksetup/fake-networksetup";

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: HashMap<String, OsString> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect();
        move |name| map.get(name).cloned()
    }

    fn good_env() -> Vec<(&'static str, &'static str)> {
        vec![
            ("IHWIP_HELPER_SOCKET", "/tmp/ihwip-test/h.sock"),
            ("IHWIP_HELPER_JOURNAL", "/tmp/ihwip-test/journal.json"),
            ("IHWIP_NETWORKSETUP", FAKE),
        ]
    }

    fn with(name: &'static str, value: &'static str) -> Vec<(&'static str, &'static str)> {
        let mut env = good_env();
        for pair in env.iter_mut() {
            if pair.0 == name {
                pair.1 = value;
            }
        }
        env
    }

    #[test]
    fn debug_config_reads_all_three_paths() {
        let c = config_from_env(env_of(&good_env())).unwrap();
        assert_eq!(c.socket, PathBuf::from("/tmp/ihwip-test/h.sock"));
        assert_eq!(c.journal, PathBuf::from("/tmp/ihwip-test/journal.json"));
        assert_eq!(c.networksetup, FAKE);
    }

    #[test]
    fn debug_config_requires_each_var_set_and_absolute() {
        for name in [
            "IHWIP_HELPER_SOCKET",
            "IHWIP_HELPER_JOURNAL",
            "IHWIP_NETWORKSETUP",
        ] {
            let missing: Vec<_> = good_env().into_iter().filter(|p| p.0 != name).collect();
            let err = config_from_env(env_of(&missing)).err().unwrap();
            assert!(err.contains(name), "{}", err);

            let err = config_from_env(env_of(&with(name, ""))).err().unwrap();
            assert!(err.contains(name), "{}", err);

            let err = config_from_env(env_of(&with(name, "relative/path")))
                .err()
                .unwrap();
            assert!(err.contains("absolute"), "{}", err);
        }
    }

    #[test]
    fn debug_config_refuses_the_real_socket() {
        for socket in [
            RELEASE_SOCKET,
            "/private/var/run/com.ithurtswhenip.helper.sock",
            "/var/run/other.sock",
            "/tmp/x/com.ithurtswhenip.helper.sock",
        ] {
            let err = config_from_env(env_of(&with("IHWIP_HELPER_SOCKET", socket)))
                .err()
                .unwrap_or_else(|| panic!("{} accepted", socket));
            assert!(err.contains("real helper socket"), "{}", err);
        }
    }

    #[test]
    fn debug_config_refuses_the_real_networksetup() {
        for ns in [
            "/usr/sbin/networksetup",
            "/usr/sbin/../sbin/networksetup",
            "/tmp/copy/networksetup",
        ] {
            let err = config_from_env(env_of(&with("IHWIP_NETWORKSETUP", ns)))
                .err()
                .unwrap_or_else(|| panic!("{} accepted", ns));
            assert!(err.contains("real networksetup"), "{}", err);
        }
    }

    #[test]
    fn tests_do_not_run_as_root() {
        assert_eq!(running_as_root(), Ok(false));
    }

    #[test]
    fn open_socket_serves_0660_and_replaces_a_stale_socket() {
        let dir = TempDir::new();
        let path = dir.0.join("h.sock");
        // A stale socket from a previous run.
        drop(UnixListener::bind(&path).unwrap());
        assert!(fs::symlink_metadata(&path).unwrap().file_type().is_socket());

        let listener = open_socket(&path).unwrap();
        let meta = fs::symlink_metadata(&path).unwrap();
        assert!(meta.file_type().is_socket());
        assert_eq!(meta.mode() & MODE_BITS, 0o660);
        UnixStream::connect(&path).unwrap();
        drop(listener);
    }

    #[test]
    fn open_socket_leaves_a_non_socket_alone_and_says_what_it_found() {
        let dir = TempDir::new();
        let path = dir.0.join("h.sock");
        fs::write(&path, "keep me").unwrap();

        let err = open_socket(&path).err().unwrap();
        assert!(err.contains("regular file"), "{}", err);
        assert!(err.contains(&path.display().to_string()), "{}", err);
        assert_eq!(fs::read_to_string(&path).unwrap(), "keep me");

        let dir_path = dir.0.join("d.sock");
        fs::create_dir(&dir_path).unwrap();
        let err = open_socket(&dir_path).err().unwrap();
        assert!(err.contains("directory"), "{}", err);
        assert!(dir_path.is_dir());
    }

    #[test]
    fn missing_socket_path_is_fine() {
        let dir = TempDir::new();
        assert_eq!(remove_stale_socket(&dir.0.join("none.sock")), Ok(()));
    }
}

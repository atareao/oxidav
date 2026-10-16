//! Shared integration-test harness: throwaway SQLite databases.
//!
//! # Incident this module exists to fix
//!
//! Before this module, **8 integration-test binaries** each carried their own
//! copy of `temp_db_url`: a helper that built throwaway SQLite DSNs under a
//! *hardcoded* path (`/tmp/opencode/oxidav-tests`, a tmpfs backed by RAM). The
//! filename embedded `std::process::id()`, so every run of the suite created a
//! fresh batch of databases that nobody ever deleted, and `remove_file(&path)`
//! only removed the `.db`, leaving the `-wal` and `-shm` siblings behind.
//!
//! Over time that tmpfs filled up (measured: 156 671 files / ~24 GB) and SQLite
//! started failing with `SqliteError { code: 778, "disk I/O error" }`, which
//! surfaced as **212 tests falsely red** — green logic, exhausted environment.
//!
//! # Design
//!
//! 1. The directory is resolved in a *pure*, testable way (explicit override >
//!    `OXIDAV_TEST_DIR` when non-empty > `std::env::temp_dir()`), so it is
//!    redirectable and respects `TMPDIR`.
//! 2. `remove_db_family` always drops the `.db` **and** its `-wal` / `-shm`
//!    siblings, so no orphaned sidecar files accumulate.
//! 3. `sweep_stale` reaps databases left behind by *dead* processes, once per
//!    process, while **never** touching a live PID (test binaries run in
//!    parallel under `cargo nextest`).
#![allow(dead_code)] // shared module: every test binary uses a different subset

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Files older than this are treated as leftovers even when their owning PID is
/// still alive (the PID was recycled onto an unrelated process).
pub const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// Resolve the throwaway-database directory, purely.
///
/// Precedence, in strict order:
/// 1. `explicit` (wins over everything);
/// 2. `env`, but only when it is defined **and non-empty**;
/// 3. `temp.join("oxidav-tests")`.
pub fn resolve_dir_from(explicit: Option<&Path>, env: Option<&OsStr>, temp: &Path) -> PathBuf {
    if let Some(dir) = explicit {
        return dir.to_path_buf();
    }
    // Let-chain (edition 2024): avoids `clippy::collapsible_if`.
    if let Some(value) = env
        && !value.is_empty()
    {
        return PathBuf::from(value);
    }
    temp.join("oxidav-tests")
}

/// The real resolution: `create_dir_all` plus a **once-per-process** sweep of
/// leftovers from dead test runs.
///
/// `OXIDAV_TEST_DIR` overrides the directory; `std::env::temp_dir()` (which
/// honours `TMPDIR`) is the fallback.
pub fn resolve_dir() -> PathBuf {
    static SWEEP: Once = Once::new();

    let dir = resolve_dir_from(
        None,
        std::env::var_os("OXIDAV_TEST_DIR").as_deref(),
        &std::env::temp_dir(),
    );
    std::fs::create_dir_all(&dir).expect("create temp test dir");

    SWEEP.call_once(|| {
        let removed = sweep_stale(&dir);
        if removed > 0 {
            eprintln!(
                "oxidav-test-harness: swept {removed} stale test file(s) from {}",
                dir.display()
            );
        }
    });

    dir
}

/// Remove a database **and its `-wal` / `-shm` siblings**.
///
/// The sibling names are built by *appending the suffix to the file name's
/// `OsStr`* (never by reformatting the path), so non-UTF-8 paths are preserved.
pub fn remove_db_family(path: &Path) {
    let _ = std::fs::remove_file(path);

    let Some(name) = path.file_name() else {
        return;
    };
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    for suffix in ["-wal", "-shm"] {
        let mut sibling = name.to_os_string();
        sibling.push(suffix);
        let _ = std::fs::remove_file(parent.join(sibling));
    }
}

/// Extract the PID embedded in a harness file name.
///
/// The naming pattern is `<prefix>-<tag>-<pid>-<n>.db` (the `tag` may itself
/// contain dashes). A trailing `-wal` / `-shm` is stripped first, `.db` is
/// required, and the PID is read **counting from the right**: the penultimate
/// field of `rsplit('-')`. Anything that does not fit returns `None`.
fn embedded_pid(file_name: &str) -> Option<u32> {
    let stem = file_name
        .strip_suffix("-wal")
        .or_else(|| file_name.strip_suffix("-shm"))
        .unwrap_or(file_name);
    let stem = stem.strip_suffix(".db")?;

    let mut fields = stem.rsplit('-');
    let _n = fields.next()?; // `<n>`
    fields.next()?.parse().ok() // `<pid>`
}

/// Is `pid` a live process?
///
/// On Linux we ask `/proc`. Where `/proc` does not exist (non-Linux hosts) we
/// cannot know, so we assume the process is **alive** and therefore never delete
/// its files on that basis.
fn pid_is_alive(pid: u32) -> bool {
    if !Path::new("/proc").exists() {
        return true;
    }
    Path::new(&format!("/proc/{pid}")).exists()
}

/// Reap leftover harness files under `dir`, returning how many were removed.
///
/// A file is deleted when either:
/// - its embedded owner PID is dead (a leftover from a previous run); or
/// - the PID is alive **but** the file is older than [`STALE_AFTER`] (the PID
///   was recycled: the current holder is not the process that wrote it).
///
/// A file whose PID is alive **and recent is never touched**: `cargo nextest`
/// runs test binaries in parallel, so a cross sweep would otherwise delete
/// another still-running process' database. A missing or unreadable directory
/// yields `0` without panicking.
pub fn sweep_stale(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };

    let now = SystemTime::now();
    let mut removed = 0;

    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }

        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        let Some(pid) = embedded_pid(name) else {
            continue;
        };

        let stale = if !pid_is_alive(pid) {
            // Owner is gone: this is debris from an earlier run.
            true
        } else {
            // PID alive: only a recycled PID explains an old file. Recent files
            // belong to a live, parallel test binary and must be left alone.
            match entry.metadata().and_then(|meta| meta.modified()) {
                Ok(modified) => now
                    .duration_since(modified)
                    .map(|age| age > STALE_AFTER)
                    .unwrap_or(false),
                Err(_) => false,
            }
        };

        if stale && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }

    removed
}

/// A throwaway file-backed DSN in the resolved directory.
pub fn temp_db_url(prefix: &str, tag: &str) -> String {
    temp_db_url_in(&resolve_dir(), prefix, tag)
}

/// Same, in an explicit directory.
pub fn temp_db_url_in(dir: &Path, prefix: &str, tag: &str) -> String {
    std::fs::create_dir_all(dir).expect("create temp test dir");
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("{prefix}-{tag}-{}-{n}.db", std::process::id()));
    remove_db_family(&path);
    format!("sqlite://{}", path.display())
}

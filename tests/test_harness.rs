//! RED-phase tests for the shared integration-test harness (`tests/common`).
//!
//! # Why these tests exist
//!
//! Before this harness, **8 integration-test binaries** each carried their own
//! copy of `temp_db_url`: a helper that built throwaway SQLite DSNs under a
//! *hardcoded* path (`/tmp/opencode/oxidav-tests`, a tmpfs backed by RAM). The
//! filename embedded `std::process::id()`, so every run of the suite created a
//! fresh batch of databases that nobody ever deleted, and `remove_file(&path)`
//! only removed the `.db`, leaving the `-wal` and `-shm` siblings behind.
//!
//! Over time the tmpfs filled up (measured: 156 671 files / ~24 GB), and SQLite
//! started failing with `SqliteError { code: 778, "disk I/O error" }`. That
//! manifested as **212 tests falsely red** — green logic, red environment.
//!
//! The replacement harness must:
//! 1. Resolve its directory in a *pure*, testable way (explicit > env override
//!    when non-empty > `std::env::temp_dir()`), so it is redirectable.
//! 2. Remove a database *and* its WAL/SHM siblings.
//! 3. Sweep orphaned databases whose owning PID is dead — while **never**
//!    touching a live PID (test binaries run in parallel under `cargo nextest`).
//!
//! These tests are deliberately written against the harness API and run against
//! the naive STUB implementation of `tests/common/mod.rs`: they are expected to
//! FAIL until the stub is replaced by the real implementation (Red → Green).
//!
//! Scenarios 1–4 and 7–9 discriminate against the stub and must fail. Scenarios
//! 5, 6, 10, 11 and 12 assert behaviour the stub happens to satisfy; they pin
//! the contract so a future implementation cannot regress it.

mod common;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A per-test working directory under `std::env::temp_dir()`, removed on drop
/// (including when an assertion panics). Its name embeds this process' PID so
/// parallel test binaries never collide.
struct WorkDir(PathBuf);

impl WorkDir {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("oxidav-harness-{}-{tag}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create test work dir");
        WorkDir(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// --- 1. Explicit directory wins over everything -------------------------------

#[test]
fn resolve_dir_from_prefers_explicit_over_everything() {
    let explicit = Path::new("/explícito");
    let got = common::resolve_dir_from(
        Some(explicit),
        Some(OsStr::new("/entorno")),
        Path::new("/temporal"),
    );
    assert_eq!(got, PathBuf::from("/explícito"));
}

// --- 2. Env override beats the temp fallback ----------------------------------

#[test]
fn resolve_dir_from_prefers_env_override_over_temp() {
    let got = common::resolve_dir_from(None, Some(OsStr::new("/entorno")), Path::new("/temporal"));
    assert_eq!(got, PathBuf::from("/entorno"));
}

// --- 3. Empty env is ignored (contract: defined *and non-empty*) --------------

#[test]
fn resolve_dir_from_ignores_empty_env() {
    let got = common::resolve_dir_from(None, Some(OsStr::new("")), Path::new("/temporal"));
    assert_eq!(got, Path::new("/temporal").join("oxidav-tests"));
}

// --- 4. Fallback to temp_dir, same `temp` as scenario 2 so the difference is
//         unambiguously the env var ------------------------------------------

#[test]
fn resolve_dir_from_falls_back_to_temp() {
    let temp = Path::new("/temporal");
    let got = common::resolve_dir_from(None, None, temp);
    assert_eq!(got, temp.join("oxidav-tests"));
}

// --- 5. URL shape -------------------------------------------------------------

#[test]
fn temp_db_url_in_has_expected_shape() {
    let dir = WorkDir::new("fmt");
    let url = common::temp_db_url_in(dir.path(), "harness", "fmt");

    assert!(url.starts_with("sqlite://"), "missing scheme: {url}");
    let raw = url.strip_prefix("sqlite://").expect("strip scheme");
    let path = Path::new(raw);

    assert!(path.is_absolute(), "path is not absolute: {raw}");

    let name = path
        .file_name()
        .expect("has file name")
        .to_string_lossy()
        .into_owned();
    assert!(name.contains("harness"), "name lacks prefix: {name}");
    assert!(name.ends_with(".db"), "name lacks .db suffix: {name}");
}

// --- 6. The target directory is created on demand -----------------------------

#[test]
fn temp_db_url_in_creates_missing_directory() {
    let base = WorkDir::new("mkdir");
    let nested = base.path().join("a").join("b");
    assert!(!nested.exists(), "precondition: nested dir must not exist");

    let _url = common::temp_db_url_in(&nested, "harness", "mk");

    assert!(nested.is_dir(), "nested dir was not created");
}

// --- 7. remove_db_family removes the `.db` and its WAL/SHM siblings -----------

#[test]
fn remove_db_family_removes_db_wal_and_shm() {
    let dir = WorkDir::new("family");
    let db = dir.path().join("x.db");
    let wal = dir.path().join("x.db-wal");
    let shm = dir.path().join("x.db-shm");
    std::fs::write(&db, b"").expect("seed db");
    std::fs::write(&wal, b"").expect("seed wal");
    std::fs::write(&shm, b"").expect("seed shm");

    common::remove_db_family(&db);

    assert!(!db.exists(), "db survived");
    assert!(!wal.exists(), "wal survived");
    assert!(!shm.exists(), "shm survived");
}

// --- 8. sweep_stale reaps orphans of a dead PID -------------------------------

/// Pick a PID that is provably dead on this host, by checking that `/proc/<pid>`
/// does not exist. `pid_max` defaults to 4 194 304 on Linux, so we start high
/// and walk upwards; the first PID without a `/proc` entry is our orphan.
fn pick_dead_pid() -> u32 {
    for pid in (4_000_000_u32..).take(1000) {
        if !Path::new(&format!("/proc/{pid}")).exists() {
            return pid;
        }
    }
    panic!("could not find a dead PID to seed the orphan scenario");
}

#[test]
fn sweep_stale_removes_dead_pid_family_and_counts_files() {
    let dir = WorkDir::new("sweep-dead");
    let pid = pick_dead_pid();
    // Documented choice: `pid` was verified above to have no `/proc/<pid>` entry,
    // i.e. it is not a live process. `api-dead-<pid>-1.db` therefore models a
    // leftover from a previous, long-gone test run.
    let db = dir.path().join(format!("api-dead-{pid}-1.db"));
    let wal = dir.path().join(format!("api-dead-{pid}-1.db-wal"));
    let shm = dir.path().join(format!("api-dead-{pid}-1.db-shm"));
    std::fs::write(&db, b"").expect("seed db");
    std::fs::write(&wal, b"").expect("seed wal");
    std::fs::write(&shm, b"").expect("seed shm");

    let removed = common::sweep_stale(dir.path());

    assert!(!db.exists(), "orphan db survived");
    assert!(!wal.exists(), "orphan wal survived");
    assert!(!shm.exists(), "orphan shm survived");
    assert_eq!(removed, 3, "expected 3 files reaped");
}

// --- 9. sweep_stale must NEVER touch a live PID -------------------------------
// Critical: test binaries run in parallel (e.g. `cargo nextest`); reaping the
// active PID's database would sabotage another running test.
//
// Both cases live in a *single* directory so the test can actually discriminate
// against a no-op sweep: a dead PID's family must disappear **and** the return
// value must be non-zero, while the live PID's fresh database must survive. A
// no-op sweep would leave the dead files in place and return 0, failing here.

#[test]
fn sweep_stale_reaps_dead_pid_and_spares_live_pid() {
    let dir = WorkDir::new("sweep-mixed");
    let dead = pick_dead_pid();
    let live = std::process::id();

    // Dead PID: verified absent from `/proc` by `pick_dead_pid`, so this models a
    // leftover from a long-gone test run.
    let dead_db = dir.path().join(format!("api-dead-{dead}-1.db"));
    let dead_wal = dir.path().join(format!("api-dead-{dead}-1.db-wal"));
    let dead_shm = dir.path().join(format!("api-dead-{dead}-1.db-shm"));
    std::fs::write(&dead_db, b"").expect("seed dead db");
    std::fs::write(&dead_wal, b"").expect("seed dead wal");
    std::fs::write(&dead_shm, b"").expect("seed dead shm");

    // Live PID: the current process, freshly written — must never be reaped.
    let live_db = dir.path().join(format!("api-live-{live}-1.db"));
    std::fs::write(&live_db, b"").expect("seed live db");

    let removed = common::sweep_stale(dir.path());

    assert!(!dead_db.exists(), "orphan db survived");
    assert!(!dead_wal.exists(), "orphan wal survived");
    assert!(!dead_shm.exists(), "orphan shm survived");
    assert!(
        live_db.exists(),
        "sweep_stale deleted a live PID's database"
    );
    assert_eq!(
        removed, 3,
        "expected only the dead PID's three files reaped"
    );
}

// --- 10. sweep_stale touches only its own naming pattern ----------------------

#[test]
fn sweep_stale_leaves_foreign_files_alone() {
    let dir = WorkDir::new("sweep-foreign");
    let notes = dir.path().join("notas.txt");
    let data = dir.path().join("datos.md");
    std::fs::write(&notes, b"hola").expect("seed notas");
    std::fs::write(&data, b"# hola").expect("seed datos");

    let _ = common::sweep_stale(dir.path());

    assert!(notes.exists(), "sweep_stale removed notas.txt");
    assert!(data.exists(), "sweep_stale removed datos.md");
}

// --- 11. sweep_stale tolerates a missing directory ----------------------------

#[test]
fn sweep_stale_on_missing_dir_returns_zero() {
    let dir = WorkDir::new("sweep-missing");
    let missing = dir.path().join("no-existe");

    assert_eq!(common::sweep_stale(&missing), 0);
}

// --- 12. sweep_stale on an empty directory ------------------------------------

#[test]
fn sweep_stale_on_empty_dir_returns_zero() {
    let dir = WorkDir::new("sweep-empty");

    assert_eq!(common::sweep_stale(dir.path()), 0);
}

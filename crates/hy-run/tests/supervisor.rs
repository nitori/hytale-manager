//! Supervisor behaviour, driven by a stub standing in for `java`.
//!
//! A real server cannot be made to exit 8, or to crash within the post-update window, on
//! demand — so the loop is tested against a script whose exit codes are scripted.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use hy_instance::Instance;
use hy_run::{NoReporter, RunOptions};

/// A `game/` layout complete enough for `command::build` to accept.
fn instance(root: &Path) -> Instance {
    std::fs::create_dir_all(root.join("Server")).unwrap();
    std::fs::write(root.join("Assets.zip"), b"assets").unwrap();
    std::fs::write(root.join("Server/HytaleServer.jar"), b"old jar").unwrap();
    std::fs::write(root.join("start.sh"), b"").unwrap();
    std::fs::write(root.join("hytale.toml"), "[java]\nversion = \">=25\"\n").unwrap();
    Instance::at(root).unwrap()
}

/// A stand-in for `java` that exits with `codes[n]` on its n-th invocation, and records the
/// arguments it was given.
fn stub_java(dir: &Path, codes: &[i32]) -> PathBuf {
    let path = dir.join("java-stub");
    let counter = dir.join("attempts");
    let argv = dir.join("argv");

    let mut cases = String::new();
    for (index, code) in codes.iter().enumerate() {
        cases.push_str(&format!("  {}) exit {code};;\n", index + 1));
    }
    cases.push_str(&format!(
        "  *) exit {};;\n",
        codes.last().copied().unwrap_or(0)
    ));

    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\n\
             n=$(cat {counter:?} 2>/dev/null || echo 0)\n\
             n=$((n+1))\n\
             echo $n > {counter:?}\n\
             echo \"$@\" >> {argv:?}\n\
             pwd >> {argv:?}\n\
             case $n in\n{cases}esac\n",
            counter = counter.display().to_string(),
            argv = argv.display().to_string(),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// The test harness's stdin is not the server's to consume, so `new` leaves it alone.
fn options(java: PathBuf) -> RunOptions {
    RunOptions::new(java)
}

fn stage_update(root: &Path, jar: &[u8]) {
    std::fs::create_dir_all(root.join("updater/staging/Server")).unwrap();
    std::fs::write(root.join("updater/staging/Server/HytaleServer.jar"), jar).unwrap();
}

#[tokio::test]
async fn exit_8_restarts_and_any_other_code_stops() {
    let dir = tempfile::tempdir().unwrap();
    let instance = instance(dir.path());
    let java = stub_java(dir.path(), &[8, 8, 0]);

    let outcome = hy_run::run(&instance, &options(java), &NoReporter)
        .await
        .unwrap();

    assert_eq!(outcome.code, 0);
    assert_eq!(outcome.restarts, 2);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("attempts"))
            .unwrap()
            .trim(),
        "3"
    );
}

#[tokio::test]
async fn a_clean_exit_does_not_restart() {
    let dir = tempfile::tempdir().unwrap();
    let instance = instance(dir.path());
    let java = stub_java(dir.path(), &[0]);

    let outcome = hy_run::run(&instance, &options(java), &NoReporter)
        .await
        .unwrap();

    assert_eq!(outcome.restarts, 0);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("attempts"))
            .unwrap()
            .trim(),
        "1"
    );
}

#[tokio::test]
async fn a_nonzero_exit_is_propagated() {
    let dir = tempfile::tempdir().unwrap();
    let instance = instance(dir.path());
    let java = stub_java(dir.path(), &[3]);

    let outcome = hy_run::run(&instance, &options(java), &NoReporter)
        .await
        .unwrap();

    assert_eq!(outcome.code, 3);
    assert!(!outcome.suspect_update);
}

#[tokio::test]
async fn the_server_runs_from_the_server_directory() {
    let dir = tempfile::tempdir().unwrap();
    let instance = instance(dir.path());
    let java = stub_java(dir.path(), &[0]);

    hy_run::run(&instance, &options(java), &NoReporter)
        .await
        .unwrap();

    let recorded = std::fs::read_to_string(dir.path().join("argv")).unwrap();
    let mut lines = recorded.lines();
    assert!(
        lines
            .next()
            .unwrap()
            .contains("-jar HytaleServer.jar --assets ../Assets.zip")
    );
    // The updater stays disabled unless the process starts from `Server/`.
    assert!(lines.next().unwrap().ends_with("/Server"));
}

#[tokio::test]
async fn a_staged_update_is_applied_before_starting() {
    let dir = tempfile::tempdir().unwrap();
    let instance = instance(dir.path());
    stage_update(dir.path(), b"new jar");
    let java = stub_java(dir.path(), &[0]);

    hy_run::run(&instance, &options(java), &NoReporter)
        .await
        .unwrap();

    assert_eq!(
        std::fs::read(instance.layout().jar()).unwrap(),
        b"new jar",
        "the update must land before the JVM starts, not after"
    );
}

#[tokio::test]
async fn a_crash_soon_after_an_update_is_flagged() {
    let dir = tempfile::tempdir().unwrap();
    let instance = instance(dir.path());
    stage_update(dir.path(), b"broken jar");
    let java = stub_java(dir.path(), &[1]);

    let outcome = hy_run::run(&instance, &options(java), &NoReporter)
        .await
        .unwrap();

    assert_eq!(outcome.code, 1);
    assert!(outcome.suspect_update);
}

#[tokio::test]
async fn a_crash_without_an_update_is_not_flagged() {
    let dir = tempfile::tempdir().unwrap();
    let instance = instance(dir.path());
    let java = stub_java(dir.path(), &[1]);

    let outcome = hy_run::run(&instance, &options(java), &NoReporter)
        .await
        .unwrap();

    assert!(!outcome.suspect_update);
}

#[tokio::test]
async fn a_restart_re_applies_staging_each_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let instance = instance(dir.path());
    let java = stub_java(dir.path(), &[8, 0]);
    stage_update(dir.path(), b"first");

    hy_run::run(&instance, &options(java), &NoReporter)
        .await
        .unwrap();

    // Staging is consumed by the first cycle, so the second must not re-apply a stale copy.
    assert_eq!(std::fs::read(instance.layout().jar()).unwrap(), b"first");
    assert!(!instance.layout().staging().exists());
}

#[tokio::test]
async fn a_second_run_is_refused_while_one_holds_the_lock() {
    let dir = tempfile::tempdir().unwrap();
    let instance = instance(dir.path());
    let _held = hy_run::RunLock::acquire(dir.path()).unwrap();
    let java = stub_java(dir.path(), &[0]);

    let error = hy_run::run(&instance, &options(java), &NoReporter)
        .await
        .unwrap_err();

    assert!(matches!(error, hy_run::Error::AlreadyRunning(_)));
    // Two JVMs on one universe/ corrupt it, so nothing should have started.
    assert!(!dir.path().join("attempts").exists());
}

#[tokio::test]
async fn a_missing_jar_fails_before_taking_the_lock() {
    let dir = tempfile::tempdir().unwrap();
    let instance = instance(dir.path());
    std::fs::remove_file(instance.layout().jar()).unwrap();
    let java = stub_java(dir.path(), &[0]);

    let error = hy_run::run(&instance, &options(java), &NoReporter)
        .await
        .unwrap_err();

    assert!(matches!(error, hy_run::Error::MissingJar(_)));
}

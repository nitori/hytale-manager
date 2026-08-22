//! Graceful shutdown.
//!
//! Deliberately the only test in this binary. Signals are delivered process-wide, so every
//! tokio listener in the process observes them — running this alongside other supervisor
//! tests makes them see a stop request that was never meant for them.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use hy_instance::Instance;
use hy_run::{NoReporter, RunOptions};

fn instance(root: &Path) -> Instance {
    std::fs::create_dir_all(root.join("Server")).unwrap();
    std::fs::write(root.join("Assets.zip"), b"assets").unwrap();
    std::fs::write(root.join("Server/HytaleServer.jar"), b"jar").unwrap();
    std::fs::write(root.join("start.sh"), b"").unwrap();
    std::fs::write(root.join("hytale.toml"), "").unwrap();
    Instance::at(root).unwrap()
}

/// A requested stop must let the server finish saving, and must not be reported as a
/// failure afterwards.
///
/// Regression: on Windows `request_stop` called `TerminateProcess`, killing the JVM
/// mid-shutdown — no hooks ran, the world was not saved, and `hy` exited 1.
#[tokio::test]
async fn a_requested_stop_waits_for_the_server_then_reports_success() {
    let dir = tempfile::tempdir().unwrap();
    let instance = instance(dir.path());
    let saved = dir.path().join("saved");
    let ready = dir.path().join("ready");

    // Stands in for a server whose console is not reading — so the `shutdown` command is
    // written but ignored, and only the signal fallback actually stops it.
    let console = dir.path().join("console");
    let java = dir.path().join("java-stub");
    std::fs::write(
        &java,
        format!(
            // `sh` points a background job's stdin at /dev/null, so the console reader
            // needs its own duplicate of the real one.
            "#!/bin/sh\n\
             exec 3<&0\n\
             trap 'sleep 1; echo done > \"{saved}\"; exit 130' TERM INT\n\
             (while read -r line <&3; do echo \"$line\" >> \"{console}\"; done) &\n\
             echo ready > \"{ready}\"\n\
             while true; do sleep 0.1; done\n",
            saved = saved.display(),
            console = console.display(),
            ready = ready.display(),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&java, std::fs::Permissions::from_mode(0o755)).unwrap();

    let options = RunOptions::new(java);
    let supervised = tokio::spawn(async move {
        hy_run::run(&instance, &options, &NoReporter).await
    });

    while !ready.exists() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // Aimed at `hy` alone, as `kill` or systemd would; the child is reached by forwarding.
    unsafe { libc::kill(std::process::id() as i32, libc::SIGTERM) };

    let outcome = supervised.await.unwrap().unwrap();

    assert!(
        saved.exists(),
        "the supervisor returned before the server finished saving"
    );
    // The console command is tried first, and reaches the server even when it is ignored:
    // in a terminal that never raises a control event this is the only thing that works.
    assert_eq!(
        std::fs::read_to_string(&console).unwrap_or_default().trim(),
        "shutdown"
    );
    assert!(outcome.stopped_by_request);
    assert_eq!(outcome.code, 130, "the server's own code is still reported");
    assert!(!outcome.suspect_update);
}

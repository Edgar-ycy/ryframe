use std::{
    fs,
    process::{Command, Stdio},
};

use super::process::{ChildGroup, run, with_process_log};

#[test]
fn windows_job_object_accepts_and_waits_for_child() {
    let group = ChildGroup::new().unwrap();
    let mut command = Command::new("cmd");
    command
        .args(["/C", "exit", "0"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let status = group.spawn(&mut command).unwrap().wait().unwrap();
    assert!(status.success());
}

#[test]
fn parallel_task_log_captures_child_output() {
    let path = std::env::temp_dir().join(format!(
        "ryframe-xtask-process-log-{}.txt",
        std::process::id()
    ));
    with_process_log("rustc-version", &path, || {
        run(std::path::Path::new("."), "rustc", &["--version"])
    })
    .unwrap();
    let log = fs::read_to_string(&path).unwrap();
    assert!(log.contains("rustc"));
    fs::remove_file(path).unwrap();
}

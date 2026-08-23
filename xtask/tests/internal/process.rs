use std::process::{Command, Stdio};

use super::process::ChildGroup;

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

use std::{
    fs,
    process::{Command, Stdio},
};

use super::process::{
    ChildGroup, configure_pnpm_environment, process_is_running, resolved_executable, run,
    with_process_log,
};

#[test]
fn process_liveness_distinguishes_the_current_and_invalid_pid() {
    assert!(process_is_running(std::process::id()));
    assert!(!process_is_running(u32::MAX));
}

#[test]
fn python_runner_honors_the_explicit_isolated_interpreter() {
    let configured = std::ffi::OsString::from("D:/tools/ryframe-python/python.exe");
    assert_eq!(
        resolved_executable("python", Some(configured.clone())),
        configured
    );
    assert_eq!(
        resolved_executable("cargo", Some(std::ffi::OsString::from("ignored"))),
        "cargo"
    );
    assert_eq!(resolved_executable("python", None), "python");
}

#[test]
fn pnpm_commands_default_to_non_interactive_ci_mode() {
    let mut command = Command::new("pnpm");
    configure_pnpm_environment(&mut command, &[]);
    let ci = command
        .get_envs()
        .find(|(key, _)| *key == "CI")
        .and_then(|(_, value)| value)
        .and_then(|value| value.to_str());
    assert_eq!(ci, Some("true"));

    let mut overridden = Command::new("pnpm");
    configure_pnpm_environment(&mut overridden, &[("CI", "false")]);
    let ci = overridden
        .get_envs()
        .find(|(key, _)| *key == "CI")
        .and_then(|(_, value)| value)
        .and_then(|value| value.to_str());
    assert_eq!(ci, Some("false"));
}

#[test]
fn child_group_accepts_and_waits_for_child() {
    let group = ChildGroup::new().unwrap();
    #[cfg(windows)]
    let mut command = Command::new("cmd");
    #[cfg(windows)]
    command.args(["/C", "exit", "0"]);
    #[cfg(unix)]
    let mut command = Command::new("sh");
    #[cfg(unix)]
    command.args(["-c", "exit 0"]);
    command
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

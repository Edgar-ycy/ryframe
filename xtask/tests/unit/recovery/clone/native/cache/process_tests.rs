use crate::{
    Result,
    process::{ProcessCancellation, command_output_with_input_timeout, with_process_cancellation},
};
use std::{
    fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};

fn run(script: &Path, timeout: Duration) -> Result<std::process::Output> {
    let system = std::env::var("SystemRoot")?;
    let executable = Path::new(&system).join("System32/WindowsPowerShell/v1.0/powershell.exe");
    command_output_with_input_timeout(
        script.parent().unwrap(),
        executable.to_str().unwrap(),
        &[
            "-NoProfile",
            "-NonInteractive",
            "-File",
            script.to_str().unwrap(),
        ],
        &[],
        None,
        timeout,
    )
}

#[test]
fn windows_failure_preserves_exit_and_discards_private_stderr() {
    let base = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&base).unwrap();
    let directory = tempfile::tempdir_in(base).unwrap();
    let script = directory.path().join("failure.ps1");
    fs::write(
        &script,
        "[Console]::Error.WriteLine('private-secret'); exit 17\n",
    )
    .unwrap();
    let result = run(&script, Duration::from_secs(10)).unwrap();
    assert_eq!(result.status.code(), Some(17));
    let failure = super::process::ControlFailure(result.status.code());
    assert!(!failure.to_string().contains("private-secret"));
}

fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let base = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&base).unwrap();
    let directory = tempfile::tempdir_in(base).unwrap();
    let script = directory.path().join("wait.ps1");
    fs::write(&script, "[IO.File]::WriteAllText((Join-Path $PSScriptRoot 'started'), 'yes')\nStart-Sleep -Seconds 30\n[IO.File]::WriteAllText((Join-Path $PSScriptRoot 'completed'), 'yes')\n").unwrap();
    (directory, script)
}

#[test]
fn windows_timeout_reaps_owned_command_without_completion_evidence() {
    let (directory, script) = fixture();
    let started = Instant::now();
    assert!(run(&script, Duration::from_millis(1500)).is_err());
    assert!(started.elapsed() < Duration::from_secs(12));
    assert!(directory.path().join("started").exists());
    assert!(!directory.path().join("completed").exists());
}

#[test]
fn windows_cancellation_before_and_during_call_never_publishes_completion() {
    let (directory, script) = fixture();
    let cancellation = ProcessCancellation::new();
    cancellation.request();
    assert!(
        with_process_cancellation(&cancellation, || run(&script, Duration::from_secs(10))).is_err()
    );
    assert!(!directory.path().join("started").exists());
    let cancellation = ProcessCancellation::new();
    let cancel = cancellation.clone();
    let marker = directory.path().join("started");
    let request = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(8);
        while !marker.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        let observed = marker.exists();
        cancel.request();
        observed
    });
    let started = Instant::now();
    assert!(
        with_process_cancellation(&cancellation, || run(&script, Duration::from_secs(15))).is_err()
    );
    assert!(request.join().unwrap());
    assert!(started.elapsed() < Duration::from_secs(12));
    assert!(!directory.path().join("completed").exists());
}

use super::*;
use std::{path::PathBuf, sync::OnceLock};

fn directory() -> tempfile::TempDir {
    let local = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&local).unwrap();
    let directory = tempfile::tempdir_in(local).unwrap();
    fs::create_dir(directory.path().join(".local-tests")).unwrap();
    directory
}
fn fixture() -> &'static Path {
    static VALUE: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
    &VALUE
        .get_or_init(|| {
            let directory = directory();
            let executable = directory
                .path()
                .join(format!("协调器 替身{}", std::env::consts::EXE_SUFFIX));
            let mut command = Command::new("rustc");
            command
                .args([
                    "--edition",
                    "2024",
                    "-C",
                    "debuginfo=0",
                    "--crate-name",
                    "full_stack_ci_process",
                ])
                .arg(
                    crate::workspace::root_dir()
                        .join("xtask/tests/fixtures/full_stack_ci_process.rs"),
                )
                .arg("-o")
                .arg(&executable);
            if cfg!(windows) {
                command.args(["-C", "linker=rust-lld.exe"]);
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            (directory, executable)
        })
        .1
}
fn command(mode: &str, directory: &Path) -> Command {
    let mut command = Command::new(fixture());
    command.arg(mode).arg(directory);
    command
}
fn gone(directory: &Path) {
    for name in ["parent.pid", "child.pid"] {
        let path = directory.join(name);
        if !path.exists() {
            continue;
        }
        let pid = fs::read_to_string(path).unwrap().parse().unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while crate::recovery::clone::native::resources::common::process_identity(pid)
            .unwrap()
            .is_some()
        {
            assert!(Instant::now() < until);
            thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
fn captures_private_stdin_without_putting_values_in_arguments_or_errors() {
    let root = directory();
    let output = capture(
        command("echo", root.path()),
        root.path(),
        b"private-password",
        Duration::from_secs(5),
        None,
        false,
        false,
    )
    .unwrap();
    assert_eq!(output, "private-password");
    let error = capture(
        command("fail", root.path()),
        root.path(),
        b"private-password",
        Duration::from_secs(5),
        None,
        false,
        false,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("17"));
    assert!(!error.contains("private-password"));
    gone(root.path());
}
#[test]
fn cancellation_timeout_and_successful_orphan_exit_reclaim_complete_managed_tree() {
    for mode in ["cancel", "timeout", "orphan"] {
        let root = directory();
        let cancel = Arc::new(AtomicBool::new(false));
        let trigger = cancel.clone();
        let path = root.path().to_path_buf();
        let monitor = thread::spawn(move || {
            if mode == "cancel" {
                let until = Instant::now() + Duration::from_secs(10);
                while !path.join("child.pid").is_file() {
                    assert!(Instant::now() < until);
                    thread::sleep(Duration::from_millis(10));
                }
                trigger.store(true, Ordering::Release);
            }
        });
        let result = capture(
            command(
                if mode == "orphan" { "orphan" } else { "hang" },
                root.path(),
            ),
            root.path(),
            b"",
            Duration::from_secs(1),
            Some(&cancel),
            false,
            false,
        );
        monitor.join().unwrap();
        assert_eq!(result.is_ok(), mode == "orphan");
        gone(root.path());
    }
}
#[test]
fn output_limit_fails_closed_and_never_leaks_processes() {
    let root = directory();
    assert!(
        capture(
            command("flood", root.path()),
            root.path(),
            b"",
            Duration::from_secs(5),
            None,
            false,
            false,
        )
        .is_err()
    );
    gone(root.path());
}
#[test]
fn logging_redacts_nested_plaintext_credentials_and_preserves_other_evidence() {
    let value = json!({"database":{"password":"private-password"},"tenant_data":{"targets":[{"password":"tenant-secret"}]},"auth":{"jwt_secret":"jwt-key"}});
    let text = redact(
        "phase reset private-password tenant-secret jwt-key".into(),
        &value,
    );
    assert_eq!(text, "phase reset [已脱敏] [已脱敏] [已脱敏]");
}

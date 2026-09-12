use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    directory: PathBuf,
    output: PathBuf,
    frontend: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let directory = root.join(".local-tests").join(format!(
            "xtask-fixture-prepare-process-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let output = directory.join("Device fixture");
        let frontend = root.parent().unwrap().join("ryframe-vue3");
        Self {
            directory,
            output,
            frontend,
        }
    }

    fn valid(&self) -> Vec<String> {
        vec![
            "check".to_owned(),
            "recovery".to_owned(),
            "fixture".to_owned(),
            "--output-dir".to_owned(),
            self.output.to_string_lossy().into_owned(),
            "--expected-backend-sha".to_owned(),
            "a".repeat(40),
            "--expected-frontend-sha".to_owned(),
            "b".repeat(40),
            "--write".to_owned(),
            "--frontend-dir".to_owned(),
            self.frontend.to_string_lossy().into_owned(),
        ]
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.directory.exists() {
            fs::remove_dir_all(&self.directory).unwrap();
        }
    }
}

fn invoke(arguments: &[String]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(arguments)
        .env(
            "RYFRAME_REFERENCE_FIXTURE_CONTROL_PROTOCOL",
            "untrusted inherited value",
        )
        .env("PYTHONUTF8", "1")
        .output()
        .unwrap()
}

#[test]
fn invalid_arguments_exit_two_before_python_starts() {
    let fixture = Fixture::new();
    let mut missing = fixture.valid();
    missing.retain(|value| value != "--write");
    let mut outside = fixture.valid();
    outside[4] = fixture.frontend.to_string_lossy().into_owned();
    for arguments in [missing, outside] {
        let result = invoke(&arguments);
        assert_eq!(result.status.code(), Some(2), "{arguments:?}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("参数错误"));
        assert!(!String::from_utf8_lossy(&result.stdout).contains("prepare_full_stack_fixture.py"));
    }
}

#[test]
fn valid_request_replaces_inherited_protocol_and_hides_paths_from_argv() {
    let fixture = Fixture::new();
    let result = invoke(&fixture.valid());
    assert_eq!(result.status.code(), Some(1));
    let stdout = String::from_utf8(result.stdout).unwrap();
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(
        stdout.contains("scripts/prepare_full_stack_fixture.py"),
        "{stdout}"
    );
    assert!(!stdout.contains("--output-dir"), "{stdout}");
    assert!(
        !stdout.contains(fixture.output.to_str().unwrap()),
        "{stdout}"
    );
    assert!(
        stderr.contains("reference_fixture_prepare_failed"),
        "{stderr}"
    );
    assert!(!fixture.output.exists());
}

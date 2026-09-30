use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    directory: PathBuf,
    binding: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let directory = root.join(".local-tests").join(format!(
            "xtask-monitoring-process-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let binding = directory.join("binding.json");
        fs::write(&binding, b"{}\n").unwrap();
        Self { directory, binding }
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
            "RYFRAME_XTASK_RECOVERY_MONITORING",
            "untrusted inherited value",
        )
        .output()
        .unwrap()
}

fn status(binding: &std::path::Path) -> Vec<String> {
    vec![
        "check".into(),
        "recovery".into(),
        "monitoring".into(),
        "status".into(),
        "--binding".into(),
        binding.to_string_lossy().into_owned(),
    ]
}

#[test]
fn invalid_requests_exit_two_without_starting_python() {
    let fixture = Fixture::new();
    for arguments in [
        vec!["check".into(), "recovery".into(), "monitoring".into()],
        [status(&fixture.binding), vec!["--write".into()]].concat(),
        vec![
            "check".into(),
            "recovery".into(),
            "monitoring".into(),
            "status".into(),
            "--binding".into(),
            "relative.json".into(),
        ],
    ] {
        let result = invoke(&arguments);
        assert_eq!(result.status.code(), Some(2), "{arguments:?}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("参数错误"));
        assert!(!String::from_utf8_lossy(&result.stdout).contains("→ python"));
    }
}

#[test]
fn valid_request_replaces_inherited_protocol_and_hides_paths_from_argv() {
    let fixture = Fixture::new();
    let result = invoke(&status(&fixture.binding));
    assert_eq!(result.status.code(), Some(1));
    let stdout = String::from_utf8(result.stdout).unwrap();
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(
        stdout.contains("-B tools/python/restore_monitoring_delivery.py"),
        "{stdout}"
    );
    assert!(!stdout.contains("--binding"), "{stdout}");
    assert!(
        !stdout.contains(fixture.binding.to_str().unwrap()),
        "{stdout}"
    );
    assert!(!stderr.contains("monitoring_protocol_error"), "{stderr}");
}

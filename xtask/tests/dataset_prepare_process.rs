use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    directory: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let directory = root.join(".local-tests").join(format!(
            "xtask-dataset-prepare-process-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        Self { directory }
    }

    fn file(&self, name: &str, content: &[u8]) -> PathBuf {
        let path = self.directory.join(name);
        fs::write(&path, content).unwrap();
        path
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
            "RYFRAME_XTASK_RECOVERY_DATASET_PREPARE",
            "untrusted inherited value",
        )
        .output()
        .unwrap()
}

fn request(plan: &std::path::Path, preflight: &std::path::Path) -> Vec<String> {
    vec![
        "check".into(),
        "recovery".into(),
        "dataset-prepare".into(),
        "--plan".into(),
        plan.to_string_lossy().into_owned(),
        "--preflight".into(),
        preflight.to_string_lossy().into_owned(),
        "--write".into(),
    ]
}

#[test]
fn invalid_requests_exit_two_before_starting_node() {
    let fixture = Fixture::new();
    let plan = fixture.file("plan.json", b"{}\n");
    let preflight = fixture.file("preflight.json", b"{}\n");
    let valid = request(&plan, &preflight);
    for arguments in [
        valid[..valid.len() - 1].to_vec(),
        [valid.clone(), vec!["--unknown".into()]].concat(),
        vec![
            "check".into(),
            "recovery".into(),
            "dataset-prepare".into(),
            "--plan".into(),
            "relative.json".into(),
            "--preflight".into(),
            preflight.to_string_lossy().into_owned(),
            "--write".into(),
        ],
    ] {
        let result = invoke(&arguments);
        assert_eq!(result.status.code(), Some(2), "{arguments:?}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("参数错误"));
        assert!(!String::from_utf8_lossy(&result.stdout).contains("→ node"));
    }
}

#[test]
fn valid_request_replaces_inherited_protocol_and_hides_paths_from_argv() {
    let fixture = Fixture::new();
    let plan = fixture.file("计划.json", b"{}\n");
    let preflight = fixture.file("预检.json", b"{}\n");
    let result = invoke(&request(&plan, &preflight));
    assert_eq!(result.status.code(), Some(1));
    let stdout = String::from_utf8(result.stdout).unwrap();
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(
        stdout.contains("→ node tools/js/restore_reference_dataset.mjs"),
        "{stdout}"
    );
    assert!(!stdout.contains("--plan"), "{stdout}");
    assert!(!stdout.contains(plan.to_str().unwrap()), "{stdout}");
    assert!(
        !stderr.contains("dataset_prepare_protocol_error"),
        "{stderr}"
    );
}

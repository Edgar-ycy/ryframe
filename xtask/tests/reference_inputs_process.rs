//! 参考恢复公开入口只通过固定私有协议传递已核验参数。

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    directory: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = backend_root().join(".local-tests").join(format!(
            "xtask-reference-inputs-process-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        Self { directory }
    }

    fn file(&self, name: &str) -> PathBuf {
        let path = self.directory.join(name);
        fs::write(&path, b"{}\n").unwrap();
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

fn backend_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn invoke(arguments: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(arguments)
        .env(
            "RYFRAME_XTASK_RECOVERY_REFERENCE",
            "untrusted inherited reference value",
        )
        .env(
            "RYFRAME_XTASK_RECOVERY_INPUTS",
            "untrusted inherited inputs value",
        )
        .env("PYTHONUTF8", "1")
        .output()
        .unwrap()
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

fn assert_private_invocation(result: Output, script: &str, protected_paths: &[&Path]) {
    assert_eq!(result.status.code(), Some(1));
    let stdout = String::from_utf8(result.stdout).unwrap();
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(stdout.contains(script), "{stdout}");
    for path in protected_paths {
        assert!(!stdout.contains(path.to_str().unwrap()), "{stdout}");
    }
    assert!(!stdout.contains("--plan"), "{stdout}");
    assert!(!stdout.contains("--reference-plan"), "{stdout}");
    assert!(!stderr.contains("protocol_error"), "{stderr}");
}

#[test]
fn reference_request_replaces_inherited_protocol_and_hides_paths_from_argv() {
    let fixture = Fixture::new();
    let plan = fixture.file("参考 plan.json");
    let result = invoke(
        &[
            strings(&["check", "recovery", "check-dataset", "--plan"]),
            vec![plan.to_string_lossy().into_owned()],
        ]
        .concat(),
    );
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("restore_reference_failed"),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_private_invocation(result, "tools/python/restore_reference.py", &[&plan]);
}

#[test]
fn restore_inputs_request_replaces_inherited_protocol_and_hides_paths_from_argv() {
    let fixture = Fixture::new();
    let reference = fixture.file("reference.json");
    let target = fixture.file("target.json");
    let backup = fixture.file("backup.json");
    let record = fixture.file("record.json");
    let result = invoke(
        &[
            strings(&[
                "check",
                "recovery",
                "inputs",
                "bindings",
                "--reference-plan",
            ]),
            vec![reference.to_string_lossy().into_owned()],
            strings(&["--target-plan"]),
            vec![target.to_string_lossy().into_owned()],
            strings(&["--backup-receipt"]),
            vec![backup.to_string_lossy().into_owned()],
            strings(&["--record"]),
            vec![record.to_string_lossy().into_owned()],
        ]
        .concat(),
    );
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("restore_inputs_failed"),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_private_invocation(
        result,
        "tools/python/restore_input_plan.py",
        &[&reference, &target, &backup, &record],
    );
}

//! 参考恢复公开入口在原生执行前核验参数。

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

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

    fn image(&self) -> BTreeMap<OsString, Vec<u8>> {
        fs::read_dir(&self.directory)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                assert!(entry.file_type().unwrap().is_file(), "{:?}", entry.path());
                (entry.file_name(), fs::read(entry.path()).unwrap())
            })
            .collect()
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

#[test]
fn reference_commands_reject_an_invalid_plan_in_rust_without_writes() {
    let fixture = Fixture::new();
    let plan = fixture.file("参考 plan.json");
    let target = fixture.file("目标计划.json");
    let evidence = target.to_str().unwrap();
    let backup = fixture.directory.to_str().unwrap();
    for (command, extra) in [
        ("check-dataset", Vec::new()),
        ("check-existing", strings(&["--side", "target"])),
        ("dataset", strings(&["--write"])),
        ("plan", strings(&["--target-plan", evidence])),
        (
            "backup",
            strings(&[
                "--inventory",
                evidence,
                "--source-generation",
                evidence,
                "--source-export-result",
                evidence,
                "--write",
            ]),
        ),
        (
            "restore",
            strings(&[
                "--backup-root",
                backup,
                "--record",
                evidence,
                "--target-plan",
                evidence,
                "--runtime-registration",
                evidence,
                "--write",
            ]),
        ),
        (
            "copy",
            strings(&["--backup-root", backup, "--copy-id", "check", "--write"]),
        ),
        (
            "damage",
            strings(&[
                "--backup-root",
                backup,
                "--artifact",
                "object.bin",
                "--write",
            ]),
        ),
    ] {
        let mut arguments = strings(&["check", "recovery", command, "--plan"]);
        arguments.push(plan.to_string_lossy().into_owned());
        arguments.extend(extra);
        let result = invoke(&arguments);
        assert_eq!(result.status.code(), Some(1), "{command}");
        assert!(result.stdout.is_empty(), "{command}");
        let stderr = String::from_utf8(result.stderr).unwrap();
        assert!(stderr.starts_with("任务失败："), "{command}: {stderr}");
        assert!(!stderr.contains("restore_reference_failed"), "{stderr}");
        assert!(!stderr.contains("ignored-private-secret"), "{stderr}");
    }
    assert_eq!(fs::read(plan).unwrap(), b"{}\n");
    assert_eq!(fs::read(target).unwrap(), b"{}\n");
    assert_eq!(fs::read_dir(&fixture.directory).unwrap().count(), 2);
}

#[test]
fn restore_inputs_run_in_process_without_private_python_arguments() {
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
    assert_eq!(result.status.code(), Some(1));
    let stdout = String::from_utf8(result.stdout).unwrap();
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(!stdout.contains("restore_input_plan.py"), "{stdout}");
    assert!(!stderr.contains("restore_inputs_failed"), "{stderr}");
    for path in [&reference, &target, &backup, &record] {
        assert!(!stdout.contains(path.to_str().unwrap()), "{stdout}");
    }
}

fn descriptor(path: &Path) -> Value {
    let bytes = fs::read(path).unwrap();
    json!({"path":path,"bytes":bytes.len(),"sha256":hex::encode(Sha256::digest(&bytes))})
}

#[test]
fn reference_restore_rejects_copied_and_changed_registered_inputs_before_writes() {
    let fixture = Fixture::new();
    let plan = fixture.file("参考计划.json");
    let target = fixture.file("目标计划.json");
    let record = fixture.file("恢复记录.json");
    let registration = fixture.file("运行登记.json");
    let plan_copy = fixture.directory.join("参考计划副本.json");
    let target_copy = fixture.directory.join("目标计划副本.json");
    let registered_target = b"{\"revision\":1}\n";
    fs::write(&target, registered_target).unwrap();
    fs::copy(&plan, &plan_copy).unwrap();
    fs::copy(&target, &target_copy).unwrap();
    fs::write(
        &registration,
        serde_json::to_vec(&json!({
            "reference_plan":descriptor(&plan),
            "target_plan":descriptor(&target),
        }))
        .unwrap(),
    )
    .unwrap();

    let assert_rejected = |plan: &Path, target: &Path, diagnostic: &str| {
        let before = fixture.image();
        let result = invoke(&strings(&[
            "check",
            "recovery",
            "restore",
            "--plan",
            plan.to_str().unwrap(),
            "--target-plan",
            target.to_str().unwrap(),
            "--backup-root",
            fixture.directory.to_str().unwrap(),
            "--record",
            record.to_str().unwrap(),
            "--runtime-registration",
            registration.to_str().unwrap(),
            "--write",
        ]));
        let stderr = String::from_utf8(result.stderr).unwrap();
        assert_eq!(result.status.code(), Some(1), "{stderr}");
        assert!(result.stdout.is_empty());
        assert!(stderr.contains(diagnostic), "{stderr}");
        assert_eq!(fixture.image(), before);
    };

    let wrong_file = "恢复命令必须使用登记的同一计划文件";
    assert_rejected(&plan_copy, &target, wrong_file);
    assert_rejected(&plan, &target_copy, wrong_file);

    let changed_target = b"{\"revision\":2}\n";
    assert_eq!(changed_target.len(), registered_target.len());
    fs::write(&target, changed_target).unwrap();
    assert_rejected(&plan, &target, "登记目标计划与登记摘要不一致");
}

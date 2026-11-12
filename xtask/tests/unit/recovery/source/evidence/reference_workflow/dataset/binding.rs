use std::process::Command;

use sha2::{Digest, Sha256};

use super::*;

const MAINTENANCE: &str = "xtask/src/maintenance.rs";
const PRODUCT: &str = "crates/ryframe/src/lib.rs";
const RUNNER_CONTENT: &[u8] = b"// fixture runner\n";

fn git(backend: &Path, arguments: &[&str]) {
    let mut command = Command::new("git");
    command.current_dir(backend).args([
        "-c",
        "core.hooksPath=.git/disabled-hooks",
        "-c",
        "core.autocrlf=false",
    ]);
    for (name, _) in std::env::vars_os() {
        if name
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GIT_")
        {
            command.env_remove(name);
        }
    }
    let output = command.args(arguments).output().unwrap();
    assert!(
        output.status.success(),
        "Git 夹具操作失败：{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write(backend: &Path, relative: &str, bytes: &[u8]) {
    let path = backend.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

#[test]
fn dataset_binding_pins_preflight_runner_and_rejects_tool_only_source_drift() {
    let directory = super::super::super::tests::temporary();
    let backend = portable_path(&fs::canonicalize(directory.path()).unwrap());
    git(&backend, &["init", "--quiet", "--object-format=sha1"]);
    write(&backend, ".gitignore", b"/.local-tests/\n");
    write(&backend, PRODUCT, b"// fixture product\n");
    write(&backend, RUNNER, RUNNER_CONTENT);
    write(&backend, MAINTENANCE, b"// fixture maintenance source\n");
    git(&backend, &["add", "."]);
    git(
        &backend,
        &[
            "-c",
            "user.name=reference-dataset-unit",
            "-c",
            "user.email=reference-dataset-unit@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "-m",
            "fixture",
        ],
    );

    let plan = json!({
        "dataset": {"records":100000, "timeout_seconds":21600},
        "source": {"scope_id":"reference-dataset-unit"},
    });
    let plan_file = ".local-tests/reference-plan.json";
    write(&backend, plan_file, &serde_json::to_vec(&plan).unwrap());
    let plan_path = backend.join(plan_file);
    let request = json!({"plan":plan_path});
    let bound = Bound::capture(&backend, &request, &plan).unwrap();
    assert_eq!(bound.plan_path, plan_path);
    assert_eq!(bound.timeout, Duration::from_secs(21600));
    assert_eq!(
        bound.preflight,
        json!({
            "format_version":1,
            "kind":"restore-reference-dataset-preflight",
            "plan_sha256":digest::plan_hash(&plan).unwrap(),
            "side":"source",
            "scope_id":"reference-dataset-unit",
            "actions":{"business":"empty_source_verified"},
        })
    );
    assert_eq!(bound.source["clean"], true);
    assert_eq!(
        bound.tools_receipt["kind"],
        "restore-reference-dataset-tools"
    );
    assert_eq!(bound.tools_receipt["root"], json!(backend));
    assert_eq!(
        bound.tools_receipt["runner"],
        json!({
            "path":backend.join(RUNNER),
            "bytes":RUNNER_CONTENT.len(),
            "sha256":hex::encode(Sha256::digest(RUNNER_CONTENT)),
        })
    );
    let before = &bound.tools_receipt["sources"];
    assert_eq!(before["tools"]["files"], json!([MAINTENANCE, RUNNER]));
    assert_eq!(before["product"]["api"]["files"], json!([PRODUCT]));
    bound.check().unwrap();

    write(&backend, MAINTENANCE, b"// changed maintenance source\n");
    let after = source::backend_domains(&source::capture(&backend).unwrap()).unwrap();
    assert_eq!(before["product"], after["product"]);
    assert_ne!(before["tools"]["sha256"], after["tools"]["sha256"]);
    assert_eq!(
        bound.check().unwrap_err().to_string(),
        "参考数据准备期间源码或维护工具来源发生变化"
    );
}

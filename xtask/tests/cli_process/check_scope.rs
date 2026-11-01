//! 实际进程验证后端独立指标边界，以及显式跨仓检查保留前端采集。
use super::{temporary_directory, xtask_command};
use serde_json::Value;
use std::{fs, path::Path, process::Command};

#[test]
fn backend_metrics_skip_frontend_until_all_scope_is_requested() {
    let directory = temporary_directory();
    let initial = directory.path().join("backend-seed");
    let frontend = directory.path().join("前端 仓库");
    fs::create_dir(&frontend).unwrap();
    fs::write(frontend.join("tracked.txt"), "初始前端\n").unwrap();
    git(&frontend, &["init"]);
    commit(&frontend);
    prepare_backend(&initial);
    git(&initial, &["init"]);
    commit(&initial);
    let backend = directory.path().join("后端 工作区");
    fs::rename(&initial, &backend).unwrap();
    fs::write(frontend.join("tracked.txt"), "未提交前端变更\n").unwrap();
    assert!(!git(&frontend, &["status", "--porcelain"]).is_empty());
    assert!(git(&backend, &["status", "--porcelain"]).is_empty());

    let backend_metrics = check(&backend, &["check"]);
    assert_eq!(backend_metrics.get("frontend_dirty"), Some(&Value::Null));
    assert_eq!(backend_metrics["backend_dirty"], false);
    assert_eq!(backend_metrics["status"], "passed");
    assert_eq!(
        fs::read(frontend.join("tracked.txt")).unwrap(),
        "未提交前端变更\n".as_bytes()
    );

    commit(&frontend);
    assert!(git(&frontend, &["status", "--porcelain"]).is_empty());
    let all_metrics = check(&backend, &["check", "--scope", "all"]);
    assert_eq!(all_metrics["frontend_dirty"], false);
    assert_eq!(all_metrics["backend_dirty"], false);
    assert_eq!(all_metrics["status"], "passed");
    assert_eq!(
        fs::read_to_string(backend.join("target/verify/metrics.jsonl"))
            .unwrap()
            .lines()
            .count(),
        2
    );
}

fn prepare_backend(root: &Path) {
    for directory in ["src", "xtask", "architecture", "config"] {
        fs::create_dir_all(root.join(directory)).unwrap();
    }
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = 'scope-fixture'\nversion = '0.1.0'\nedition = '2024'\n\n[workspace]\n",
    )
    .unwrap();
    fs::write(
        root.join("Cargo.lock"),
        "version = 4\n\n[[package]]\nname = 'scope-fixture'\nversion = '0.1.0'\n",
    )
    .unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn marker() {}\n").unwrap();
    fs::write(root.join(".gitignore"), "/target/\n/.local-tests/\n").unwrap();
    fs::write(
        root.join("config/tools.toml"),
        "[workspace]\nfrontend_dir = '../前端 仓库'\n",
    )
    .unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("architecture/change-surface.toml"),
        root.join("architecture/change-surface.toml"),
    )
    .unwrap();
}

fn git(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .args([
            "-c",
            "user.name=维护验证夹具",
            "-c",
            "user.email=scope-fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(arguments)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn commit(root: &Path) {
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "test(check): 保存指标边界夹具"]);
}

fn check(root: &Path, arguments: &[&str]) -> Value {
    let output = xtask_command()
        .args(arguments)
        .current_dir(root.join("xtask"))
        .env_remove("GITHUB_OUTPUT")
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("任务图为空"));
    let metrics = fs::read_to_string(root.join("target/verify/metrics.jsonl")).unwrap();
    serde_json::from_str(metrics.lines().last().unwrap()).unwrap()
}

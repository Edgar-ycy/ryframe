//! 性能身份公开入口必须在启动 Node 前拒绝歧义或越界参数。

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
            "xtask-performance-identities-process-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        Self { directory }
    }

    fn environment(&self) -> PathBuf {
        let value = self.directory.join("environment.json");
        fs::write(&value, b"{}\n").unwrap();
        value
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
            "RYFRAME_PERFORMANCE_IDENTITIES_PROTOCOL",
            "untrusted inherited value",
        )
        .output()
        .unwrap()
}

#[test]
fn invalid_paths_and_missing_write_exit_two_without_starting_node() {
    let fixture = Fixture::new();
    let environment = fixture.environment();
    let output = fixture.directory.join("plan.json");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let base = [
        "data".to_owned(),
        "performance-identities".to_owned(),
        "plan".to_owned(),
        "--environment".to_owned(),
        environment.to_string_lossy().into_owned(),
        "--output".to_owned(),
        output.to_string_lossy().into_owned(),
        "--write".to_owned(),
    ];
    let mut cases = vec![base[..base.len() - 1].to_vec()];
    let mut outside = base.clone();
    outside[6] = root.join("outside.json").to_string_lossy().into_owned();
    cases.push(outside.to_vec());
    let mut relative = base;
    relative[4] = "relative.json".to_owned();
    cases.push(relative.to_vec());
    for arguments in cases {
        let result = invoke(&arguments);
        assert_eq!(result.status.code(), Some(2), "参数：{arguments:?}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("参数错误"));
        assert!(!String::from_utf8_lossy(&result.stdout).contains("→ node"));
    }
}

#[test]
fn valid_public_request_replaces_inherited_protocol_and_hides_paths_from_argv() {
    let fixture = Fixture::new();
    let environment = fixture.environment();
    let output = fixture.directory.join("计划 输出.json");
    let arguments = [
        "data".to_owned(),
        "performance-identities".to_owned(),
        "plan".to_owned(),
        "--environment".to_owned(),
        environment.to_string_lossy().into_owned(),
        "--output".to_owned(),
        output.to_string_lossy().into_owned(),
        "--write".to_owned(),
    ];
    let result = invoke(&arguments);
    assert_eq!(result.status.code(), Some(1));
    let stdout = String::from_utf8(result.stdout).unwrap();
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(
        stdout.contains("→ node tools/js/devex_prepare_identities.mjs"),
        "{stdout}"
    );
    assert!(!stdout.contains("--environment"), "{stdout}");
    assert!(!stdout.contains(environment.to_str().unwrap()), "{stdout}");
    assert!(stderr.contains("identity_preparation_failed"), "{stderr}");
    assert!(
        !stderr.contains("identity_preparation_protocol_error"),
        "{stderr}"
    );
    assert!(!output.exists());
}

use std::process::{Command, Output};

fn invoke(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(arguments)
        .env_remove("GITHUB_OUTPUT")
        .output()
        .unwrap()
}

#[test]
fn default_help_exposes_only_five_task_families_and_exits_zero() {
    let result = invoke(&[]);
    assert!(result.status.success());
    assert!(result.stderr.is_empty());
    let output = String::from_utf8(result.stdout).unwrap();
    let families = output
        .lines()
        .filter_map(|line| line.trim().strip_prefix("cargo xtask "))
        .map(|line| line.split_whitespace().next().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(families, ["dev", "check", "build", "generate", "data"]);
    for command in families {
        let result = invoke(&[command, "--help"]);
        assert!(result.status.success(), "命令：{command}");
        assert!(result.stderr.is_empty(), "命令：{command}");
    }
}

#[test]
fn dev_help_lists_every_supported_development_option() {
    let result = invoke(&["dev", "--help"]);
    assert!(result.status.success());
    assert!(result.stderr.is_empty());
    let output = String::from_utf8(result.stdout).unwrap();
    assert!(
        output.contains("cargo xtask dev [--config-dir <目录>] [--workspace-root <后端根>] [--plan] [--measure-once [--case <场景>] [--output <文件>]]"),
        "{output}"
    );
    for option in [
        "--config-dir",
        "--workspace-root",
        "--plan",
        "--measure-once",
        "--case",
        "--output",
    ] {
        assert!(output.contains(option), "{option}\n{output}");
    }
    assert!(output.contains("只管理后端 API、Worker"), "{output}");
    assert!(output.contains("corepack pnpm dev"), "{output}");
    assert!(!output.contains("Vite"), "{output}");
    assert!(!output.contains("--frontend-dir"), "{output}");
    assert!(output.contains("DevEx 保存场景"), "{output}");
}

#[test]
fn data_help_describes_typed_write_and_path_boundaries() {
    let result = invoke(&["data", "--help"]);
    assert!(result.status.success());
    assert!(result.stderr.is_empty());
    let output = String::from_utf8(result.stdout).unwrap();
    for expected in [
        "data backup inventory --output <绝对新文件>",
        "data backup register --manifest <绝对文件> --backup-root <绝对目录> --write",
        "data restore begin --plan <绝对文件>",
        "data target inventory --target <目标键>",
        "data reset execute --plan-hash <sha256> --confirm-reset <精确短语> --write",
        "备份根、隔离配置和 runner 可使用已登记的外部绝对目录",
    ] {
        assert!(output.contains(expected), "{expected}\n{output}");
    }
}

#[test]
fn check_help_lists_every_supported_performance_operation() {
    let result = invoke(&["check", "--help"]);
    assert!(result.status.success());
    assert!(result.stderr.is_empty());
    let output = String::from_utf8(result.stdout).unwrap();
    assert!(output.contains("check perf run|paired|summarize|compare"));
    assert!(output.contains("check ci frontend-source --event-name <事件>"));
    assert!(output.contains("check ci security report <cyclonedx|trivy> --input <绝对文件>"));
    assert!(output.contains("check release source --tag <tag>"));
    assert!(output.contains("check release ci --backend-repository <owner/repo>"));
    assert!(output.contains("check release ci record-pair --output <绝对文件>"));
    assert!(output.contains("发布核验直接由 Rust xtask 执行，不依赖 Python"));
    assert!(output.contains("check perf cgroup <run|cleanup>"));
    assert!(output.contains("check recovery full-stack <prepare|rate-limit|start|collect>"));
    assert!(output.contains("check language [--min-rust-percent <0..100>]"));
    assert!(output.contains("language 默认仅向 stdout 报告固定口径的原始文件字节"));
    assert!(output.contains("Cargo 在独立降权进程中启动"));
    assert!(output.contains("--plan 只输出同一任务图，不启动 sudo、不创建目录"));
    assert!(output.contains("ci integration 使用 Rust xtask 生成临时回环 CA"));
    assert!(output.contains("不调用 Python"));
    for field in [
        ".local-tests/integration/ci.toml",
        "format_version",
        "run_id",
        "artifact_root",
        "redis_config_dir",
        "openssl_program",
        "test_jobs",
        "timeout_secs",
    ] {
        assert!(output.contains(field), "{field}\n{output}");
    }
    for removed in [
        "RYFRAME_TLS_ARTIFACT_DIR",
        "RYFRAME_REDIS_HOST",
        "RYFRAME_OPENSSL",
    ] {
        assert!(!output.contains(removed), "{removed}\n{output}");
    }
}

#[test]
fn recovery_runtime_status_rejects_missing_evidence_before_python() {
    let result = invoke(&[
        "check",
        "recovery",
        "runtime",
        "status",
        "--runtime-registration",
        "missing-runtime-registration.json",
        "--target-plan",
        "missing-runtime-target-plan.json",
    ]);
    assert_eq!(result.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("参数错误："), "{stderr}");
    assert!(!stderr.contains("任务失败："));
}

#[test]
fn consumer_contract_runs_the_inline_source_gate_before_frontend_tasks() {
    // 传入实际存在的目录，确保测试验证的是仓库身份预检，而不是平台相关的路径错误。
    let frontend_dir = std::env::current_dir().unwrap();
    let frontend_dir = frontend_dir.to_string_lossy().into_owned();
    let result = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args([
            "check",
            "ci",
            "consumer-contract",
            "--frontend-dir",
            frontend_dir.as_str(),
        ])
        .env(
            "RYFRAME_CI_BACKEND_HEAD",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .env("RYFRAME_CI_BACKEND_REPOSITORY", "invalid")
        .env(
            "RYFRAME_CI_CANDIDATE_OPENAPI",
            std::env::current_dir()
                .unwrap()
                .join("openapi/openapi.json")
                .to_string_lossy()
                .into_owned(),
        )
        .env_remove("GITHUB_OUTPUT")
        .output()
        .unwrap();

    assert_eq!(result.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("期望后端仓库必须是 owner/repository"),
        "{stderr}"
    );
}

struct LanguageProcessFixture(std::path::PathBuf);

impl LanguageProcessFixture {
    fn new(label: &str) -> Self {
        let parent = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".local-tests");
        std::fs::create_dir_all(&parent).unwrap();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = parent.join(format!(
            "language-process-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(root.join("xtask")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
        std::fs::write(root.join(".gitignore"), ".local-tests/\n").unwrap();
        std::fs::write(root.join("check.py"), "print('python source')\n").unwrap();
        let result = Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(result.status.success());
        Self(root)
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
        command
            .current_dir(&self.0)
            .env_remove("RYFRAME_DEVEX_TARGET_ROOT")
            .env_remove("GITHUB_OUTPUT");
        command
    }

    fn conflicting_root_target(&self) -> std::path::PathBuf {
        let session = self.0.join(".local-tests/foreign-session");
        let backend = session.join("worktree/b");
        for root in [&self.0, &backend] {
            std::fs::create_dir_all(root.join("xtask")).unwrap();
            std::fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
            std::fs::write(root.join(".gitignore"), ".local-tests/\n").unwrap();
            std::fs::write(
                root.join("foreign_only_marker.rs"),
                "// foreign_root_rust_marker_9412dc6f\n",
            )
            .unwrap();
            std::fs::write(
                root.join("foreign_only_marker.js"),
                b"\x00foreign-root-fixture\xff",
            )
            .unwrap();
        }
        let result = Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&backend)
            .output()
            .unwrap();
        assert!(result.status.success());
        std::fs::create_dir(session.join("target")).unwrap();
        session.join("target")
    }
}

impl Drop for LanguageProcessFixture {
    fn drop(&mut self) {
        let parent = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".local-tests");
        assert_eq!(self.0.parent(), Some(parent.as_path()));
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn language_plan_runs_without_git_and_does_not_create_the_requested_output() {
    let fixture = LanguageProcessFixture::new("plan");
    let result = fixture
        .command()
        .args([
            "check",
            "language",
            "--min-rust-percent",
            "80",
            "--output",
            ".local-tests/missing/new.json",
            "--write",
            "--plan",
        ])
        .env("PATH", "")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.stderr.is_empty());
    let output = String::from_utf8(result.stdout).unwrap();
    for task in ["language.report", "language.write", "language.threshold"] {
        assert!(output.contains(task), "{output}");
    }
    assert!(!fixture.0.join(".local-tests").exists());
    assert!(!fixture.0.join("target").exists());
}

#[test]
fn language_default_stdout_is_read_only_and_explicit_threshold_exits_one() {
    let fixture = LanguageProcessFixture::new("report");
    let read_only = fixture
        .command()
        .args(["check", "language"])
        .output()
        .unwrap();
    assert!(
        read_only.status.success(),
        "{}",
        String::from_utf8_lossy(&read_only.stderr)
    );
    let output = String::from_utf8(read_only.stdout).unwrap();
    assert!(output.contains("Rust 占比：0.000000%"), "{output}");
    assert!(output.contains("未请求阈值检查"), "{output}");
    let checked = fixture
        .command()
        .args(["check", "language", "--min-rust-percent", "80"])
        .output()
        .unwrap();
    assert_eq!(checked.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&checked.stdout).contains("未通过"));
    assert!(!fixture.0.join(".local-tests").exists());
    assert!(!fixture.0.join("target").exists());
    let invalid = fixture
        .command()
        .args(["check", "language", "--output", ".local-tests/new.json"])
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));
}

#[test]
fn language_json_uses_cwd_ancestor_and_ignores_both_legacy_root_variables() {
    let current = LanguageProcessFixture::new("当前 后端");
    let foreign = LanguageProcessFixture::new("foreign 后端");
    let target = foreign.conflicting_root_target();
    let rust_source = "// current_root_rust_marker_2a97fb30\n";
    std::fs::write(current.0.join("current_only_marker.rs"), rust_source).unwrap();
    let nested = current.0.join("中文 带空格目录/深层 子目录");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::create_dir(current.0.join(".local-tests")).unwrap();
    let report_path = current.0.join(".local-tests/current-root.json");
    let result = current
        .command()
        .current_dir(&nested)
        .env("RYFRAME_WORKSPACE_ROOT", &foreign.0)
        .env("RYFRAME_DEVEX_TARGET_ROOT", &target)
        .args(["check", "language", "--output"])
        .arg(&report_path)
        .arg("--write")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let bytes = std::fs::read(&report_path).unwrap();
    let report: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let expected = [
        ".gitignore",
        "Cargo.toml",
        "check.py",
        "current_only_marker.rs",
    ];
    let files = report["files"].as_array().unwrap();
    let paths = files
        .iter()
        .map(|file| file["path"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(paths, expected);
    let total_bytes = expected
        .iter()
        .map(|path| std::fs::metadata(current.0.join(path)).unwrap().len())
        .sum::<u64>();
    assert_eq!(
        report["total"]["files"].as_u64(),
        Some(expected.len() as u64)
    );
    assert_eq!(report["total"]["bytes"].as_u64(), Some(total_bytes));
    assert_eq!(report["self_maintained"]["rust"]["files"].as_u64(), Some(1));
    assert_eq!(
        report["self_maintained"]["rust"]["bytes"].as_u64(),
        Some(rust_source.len() as u64)
    );
    assert!(files.iter().all(|file| {
        !file["path"]
            .as_str()
            .unwrap()
            .contains("foreign_only_marker")
    }));
    assert!(!foreign.0.join(".local-tests/current-root.json").exists());
    assert!(
        !target
            .parent()
            .unwrap()
            .join("worktree/b/.local-tests/current-root.json")
            .exists()
    );
    assert!(!current.0.join("target").exists());
}

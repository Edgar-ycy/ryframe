//! 通过实际二进制验证公开入口，避免解析测试遗漏 main 的退出码。

use std::{
    fs,
    process::{Command, Output},
};

fn xtask_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
}

fn invoke(arguments: &[&str]) -> Output {
    xtask_command().args(arguments).output().unwrap()
}

fn invoke_with_environment(arguments: &[&str], environment: &[(&str, &str)]) -> Output {
    xtask_command()
        .args(arguments)
        .envs(environment.iter().copied())
        .env_remove("RYFRAME_CI_FRONTEND_REF")
        .output()
        .unwrap()
}

#[test]
fn invalid_public_arguments_exit_two_before_running_tasks() {
    for arguments in [
        ["verify"].as_slice(),
        ["verify", "--help"].as_slice(),
        ["help", "unknown"].as_slice(),
        ["check", "--scope", "unknown"].as_slice(),
        ["build", "--write"].as_slice(),
        ["build", "--plan", "--plan"].as_slice(),
        ["generate", "api", "--commit", "HEAD"].as_slice(),
        ["data", "unknown"].as_slice(),
        ["check", "recovery", "runtime", "restart"].as_slice(),
        [
            "check",
            "recovery",
            "monitoring",
            "start",
            "--binding",
            "binding.json",
        ]
        .as_slice(),
        [
            "check",
            "recovery",
            "monitoring",
            "status",
            "--binding",
            "binding.json",
            "--write",
        ]
        .as_slice(),
        ["check", "ci", "required"].as_slice(),
        ["check", "ci", "security"].as_slice(),
        ["check", "ci", "security", "source", "extra"].as_slice(),
        [
            "check",
            "ci",
            "security",
            "deployment",
            "source",
            "--base",
            "base",
        ]
        .as_slice(),
        [
            "check",
            "ci",
            "security",
            "deployment",
            "image",
            "--base",
            "",
            "--head",
            "",
            "--image",
            "invalid image",
            "--expected-commit",
            "0123456789abcdef0123456789abcdef01234567",
        ]
        .as_slice(),
        [
            "check",
            "ci",
            "required",
            "--event",
            "push",
            "--needs-json",
            "not-json",
        ]
        .as_slice(),
        [
            "check",
            "ci",
            "required",
            "--event",
            "pull_request",
            "--action",
            "closed",
            "--needs-json",
            "{}",
        ]
        .as_slice(),
    ] {
        let result = invoke(arguments);
        assert_eq!(result.status.code(), Some(2), "参数：{arguments:?}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("参数错误"));
        assert!(result.stdout.is_empty());
    }
}

#[test]
fn invalid_security_report_arguments_exit_two_before_running_tasks() {
    for arguments in [
        ["check", "ci", "security", "report", "cyclonedx"].as_slice(),
        [
            "check",
            "ci",
            "security",
            "report",
            "cyclonedx",
            "--unknown",
        ]
        .as_slice(),
        [
            "check",
            "ci",
            "security",
            "report",
            "cyclonedx",
            "--input",
            "one",
            "--input",
            "two",
        ]
        .as_slice(),
        [
            "check",
            "ci",
            "security",
            "report",
            "cyclonedx",
            "--require-reproducible",
            "--require-reproducible",
        ]
        .as_slice(),
        [
            "check",
            "ci",
            "security",
            "report",
            "trivy",
            "--input",
            "relative.json",
        ]
        .as_slice(),
        [
            "check",
            "ci",
            "security",
            "report",
            "trivy",
            "--input",
            "report.json",
            "--require-reproducible",
        ]
        .as_slice(),
    ] {
        let result = invoke(arguments);
        assert_eq!(result.status.code(), Some(2), "参数：{arguments:?}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("参数错误"));
        assert!(result.stdout.is_empty());
    }
}

#[test]
fn security_source_preserves_prerequisite_failure_exit_code() {
    let result = xtask_command()
        .args(["check", "ci", "security", "source"])
        .env("RYFRAME_PYTHON", "missing-security-python-executable")
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("开始 CI 原子任务：python.environment"));
    assert!(!stdout.contains("开始 CI 原子任务：ci.security.supply-chain"));
    assert!(!stdout.contains("开始 CI 原子任务：ci.security.audit"));
}

#[test]
fn security_report_runs_only_its_registered_report_node() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let report_dir = root.join("target/cli process-供应链");
    fs::create_dir_all(&report_dir).unwrap();
    let report = report_dir.join(format!("cyclonedx-{}.json", std::process::id()));
    fs::write(
        &report,
        r#"{"bomFormat":"CycloneDX","specVersion":"1.5","metadata":{"component":{"name":"ryframe"}},"components":[{"name":"dependency"}]}"#,
    )
    .unwrap();
    let result = invoke(&[
        "check",
        "ci",
        "security",
        "report",
        "cyclonedx",
        "--input",
        report.to_str().unwrap(),
        "--require-reproducible",
    ]);
    let _ = fs::remove_file(&report);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("开始 CI 原子任务：python.environment"));
    assert!(stdout.contains("开始 CI 原子任务：ci.security.report.cyclonedx"));
    assert!(!stdout.contains("ci.security.supply-chain"));
    assert!(!stdout.contains("ci.security.audit"));
}

#[test]
fn required_ci_command_preserves_usage_and_task_failure_exit_codes() {
    let valid = r#"{"plan":{"result":"success","outputs":{"preflight":"true","rust_gate":"true","resource_gate":"true","integration":"true","consumer_contract":"false"}},"rust-gate":{"result":"success"},"resource-gate":{"result":"success"},"integration":{"result":"success"},"windows-smoke":{"result":"success"},"security-audit":{"result":"success"}}"#;
    let success = invoke(&[
        "check",
        "ci",
        "required",
        "--event",
        "push",
        "--action",
        "",
        "--needs-json",
        valid,
    ]);
    assert!(
        success.status.success(),
        "{}",
        String::from_utf8_lossy(&success.stderr)
    );
    let stdout = String::from_utf8(success.stdout).unwrap();
    assert!(
        stdout.contains("开始 CI 原子任务：ci.required-jobs"),
        "{stdout}"
    );
    assert!(stdout.contains("Required 汇总校验通过"), "{stdout}");

    let failed = valid.replacen(
        r#""rust-gate":{"result":"success"}"#,
        r#""rust-gate":{"result":"failure"}"#,
        1,
    );
    let failure = invoke(&[
        "check",
        "ci",
        "required",
        "--event",
        "push",
        "--needs-json",
        &failed,
    ]);
    assert_eq!(failure.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&failure.stderr).contains("rust-gate 期望 success，实际 failure")
    );
}

#[test]
fn recovery_help_uses_the_actual_stage_parser_without_creating_requested_output() {
    let output = std::env::temp_dir().join(format!(
        "ryframe-recovery-help-{}-未创建/output.json",
        std::process::id()
    ));
    assert!(!output.parent().unwrap().exists());
    let result = xtask_command()
        .env("PYTHONIOENCODING", "utf-8")
        .args(["check", "recovery", "runtime", "register", "--output"])
        .arg(&output)
        .arg("--help")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(text.contains("--target-plan"), "{text}");
    assert!(text.contains("--output"), "{text}");
    assert!(!output.parent().unwrap().exists());
    for (arguments, expected) in [
        (
            ["check", "recovery", "runtime", "--help"].as_slice(),
            "usage: restore_runtime.py",
        ),
        (
            ["check", "recovery", "fresh-target", "-h"].as_slice(),
            "--workspace",
        ),
        (
            ["check", "recovery", "source", "comparison-verify", "--help"].as_slice(),
            "--receipt",
        ),
        (
            ["check", "recovery", "dataset-prepare", "--help"].as_slice(),
            "--verify-existing PATH",
        ),
        (
            ["check", "recovery", "monitoring", "start", "--help"].as_slice(),
            "--binding BINDING",
        ),
    ] {
        let result = invoke_with_environment(arguments, &[("PYTHONIOENCODING", "utf-8")]);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let text = String::from_utf8(result.stdout).unwrap();
        assert!(text.contains(expected), "{text}");
    }
}

#[test]
fn build_plan_preserves_effective_parameters_without_spawning_or_writing() {
    let missing_frontend = std::env::temp_dir().join(format!(
        "ryframe-build-plan-{}-不存在 空格",
        std::process::id()
    ));
    assert!(!missing_frontend.exists());
    let missing_frontend = missing_frontend.to_string_lossy().into_owned();
    let result = xtask_command()
        .args([
            "build",
            "--profile",
            "dev",
            "--real",
            "--plan",
            "--frontend-dir",
            missing_frontend.as_str(),
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
    assert!(
        output.contains("构建计划：profile=dev，real=true"),
        "{output}"
    );
    assert!(output.contains("任务 backend-api：角色=API"), "{output}");
    assert!(
        output.contains("任务 backend-worker：角色=Worker"),
        "{output}"
    );
    assert!(output.contains("--target-dir target/build"), "{output}");
    assert!(
        output.contains("--features bin-api --bin ryframe"),
        "{output}"
    );
    assert!(
        output.contains("--features bin-worker --bin ryframe-worker"),
        "{output}"
    );
    assert!(output.contains("jobs=继承 Cargo 有效配置"), "{output}");
    assert!(output.contains("输入=Cargo 工作区清单"), "{output}");
    assert!(output.contains("角色=前端生产产物"), "{output}");
    assert!(output.contains("corepack pnpm build --real"), "{output}");
    assert!(output.contains(&missing_frontend), "{output}");
    assert!(!std::path::Path::new(&missing_frontend).exists());
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
        output.contains("cargo xtask dev [--measure-once] [--frontend-dir PATH]"),
        "{output}"
    );
    assert!(output.contains("DevEx 保存场景"), "{output}");
}

#[test]
fn check_help_lists_every_supported_performance_operation() {
    let result = invoke(&["check", "--help"]);
    assert!(result.status.success());
    assert!(result.stderr.is_empty());
    let output = String::from_utf8(result.stdout).unwrap();
    assert!(output.contains("check perf run|paired|summarize|compare"));
    assert!(output.contains("check ci security report <cyclonedx|trivy> --input <绝对文件>"));
}

#[test]
fn recovery_runtime_status_forwards_registered_lifecycle_paths() {
    let missing = "missing-runtime-target-plan.json";
    let result = invoke(&[
        "check",
        "recovery",
        "runtime",
        "status",
        "--runtime-registration",
        "missing-runtime-registration.json",
        "--target-plan",
        missing,
    ]);
    assert_eq!(result.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains(missing), "{stderr}");
    assert!(!stderr.contains("未知 recovery runtime 子操作"));
}

#[test]
fn consumer_contract_runs_the_inline_source_gate_before_frontend_tasks() {
    let result = invoke_with_environment(
        &[
            "check",
            "ci",
            "consumer-contract",
            "--frontend-dir",
            "D:/前端 worktree",
        ],
        &[
            (
                "RYFRAME_CI_BACKEND_HEAD",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
            ("RYFRAME_CI_BACKEND_REPOSITORY", "invalid"),
            (
                "RYFRAME_CI_CANDIDATE_OPENAPI",
                "D:/候选 contract/candidate-openapi.json",
            ),
        ],
    );

    assert_eq!(result.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("期望后端仓库必须是 owner/repository"),
        "{stderr}"
    );
}

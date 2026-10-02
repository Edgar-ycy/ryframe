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
        output.contains("cargo xtask dev [--measure-once]"),
        "{output}"
    );
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
        "data file <backfill-sha256|drain-legacy-reservations>",
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
    assert!(output.contains("check perf cgroup <run|cleanup>"));
    assert!(output.contains("check recovery full-stack <prepare|rate-limit|start|collect>"));
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

//! 通过实际二进制验证公开入口，避免解析测试遗漏 main 的退出码。

use std::process::{Command, Output};

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
    ] {
        let result = invoke(arguments);
        assert_eq!(result.status.code(), Some(2), "参数：{arguments:?}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("参数错误"));
        assert!(result.stdout.is_empty());
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
    assert!(output.contains("角色=API、Worker"), "{output}");
    assert!(output.contains("--target-dir target/build"), "{output}");
    assert!(output.contains("--features bin-api,bin-worker"), "{output}");
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
    assert!(
        String::from_utf8(result.stdout)
            .unwrap()
            .contains("check perf run|paired|summarize|compare")
    );
}

#[test]
fn recovery_runtime_forwards_the_global_frontend_as_one_argument() {
    let missing = "missing-runtime-receipt.json";
    let result = invoke(&[
        "check",
        "recovery",
        "runtime",
        "verify",
        "--bindings",
        "missing-bindings.json",
        "--frontend-url",
        "http://127.0.0.1:4174",
        "--receipt",
        missing,
        "--frontend-dir",
        "D:/前端 worktree",
    ]);
    assert_eq!(result.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains(missing), "{stderr}");
    assert!(!stderr.contains("the following arguments are required: --frontend-dir"));
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

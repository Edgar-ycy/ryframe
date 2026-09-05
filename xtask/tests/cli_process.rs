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
        "--backend-dir",
        ".",
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

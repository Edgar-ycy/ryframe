//! 通过实际二进制验证公开入口，避免解析测试遗漏 main 的退出码。

use std::process::{Command, Output};

fn invoke(arguments: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command.args(arguments).output().unwrap()
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

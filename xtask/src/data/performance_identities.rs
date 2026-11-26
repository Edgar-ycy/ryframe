use std::{
    io::Read,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use crate::{
    Result,
    cli::PerformanceIdentitiesCommand,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    workspace::root_dir,
};

const MAX_PROTOCOL_BYTES: usize = 32 * 1024;

#[cfg(test)]
#[path = "performance_identities/tests/fixture.rs"]
pub(crate) mod test_fixture;

#[cfg(test)]
#[path = "performance_identities/tests/execution.rs"]
mod execution_tests;
#[cfg(test)]
#[path = "performance_identities/tests/producer.rs"]
mod producer_tests;
#[cfg(test)]
#[path = "../../tests/unit/data/performance_identities/seed_evidence.rs"]
mod seed_evidence_tests;

#[path = "performance_identities/clone_bridge.rs"]
mod clone_bridge;
#[path = "performance_identities/evidence.rs"]
mod evidence;
#[path = "performance_identities/execution.rs"]
mod execution;
#[path = "performance_identities/inspection.rs"]
mod inspection;
#[path = "performance_identities/ledger.rs"]
mod ledger;
#[path = "performance_identities/native.rs"]
mod native;
#[path = "performance_identities/plan.rs"]
pub(crate) mod plan;
#[path = "performance_identities/producer.rs"]
mod producer;
#[path = "performance_identities/producer_authorization.rs"]
pub(crate) mod producer_authorization;
#[path = "performance_identities/receipt.rs"]
mod receipt;
#[path = "performance_identities/reconcile.rs"]
pub(crate) mod reconcile;
#[path = "performance_identities/run_lock.rs"]
mod run_lock;
#[path = "performance_identities/seed_evidence.rs"]
pub(crate) mod seed_evidence;
#[path = "performance_identities/state.rs"]
pub(crate) mod state;

/// 私有入口只接收有界 stdin，不接受配置环境变量或业务路径参数。
pub(crate) fn run_hidden_if_requested() -> Option<i32> {
    if let Some(code) = clone_bridge::run_hidden_if_requested() {
        return Some(code);
    }
    if let Some(code) = producer::run_hidden_if_requested() {
        return Some(code);
    }
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments.first().map(String::as_str) != Some("__prepare-identities") {
        return None;
    }
    let mut bytes = Vec::new();
    let command = if arguments.len() == 1
        && std::io::stdin()
            .take((MAX_PROTOCOL_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .is_ok()
    {
        private_command(&bytes, &root_dir())
    } else {
        Err("身份准备私有入口不接受参数".into())
    };
    Some(match command {
        Err(_) => {
            eprintln!("identity_preparation_protocol_error：身份准备私有协议无效。");
            2
        }
        Ok(command) => match run(&command) {
            Ok(()) => 0,
            Err(_) => {
                eprintln!(
                    "identity_preparation_failed：身份准备失败，请核对计划与账本；不自动重放。"
                );
                1
            }
        },
    })
}

pub(crate) fn private_command(bytes: &[u8], root: &Path) -> Result<PerformanceIdentitiesCommand> {
    if bytes.len() > MAX_PROTOCOL_BYTES {
        return Err("身份准备私有协议超过 32 KiB".into());
    }
    let source = std::str::from_utf8(bytes).map_err(|_| "身份准备私有协议不是 UTF-8")?;
    let value = crate::recovery::clone::native::json::parse_fragment(source, "身份私有协议")?;
    if value["format_version"] != 1 || value["write"] != true {
        return Err("身份协议版本或写入授权无效".into());
    }
    let path = |key| -> Result<PathBuf> {
        let value = value[key].as_str().ok_or("身份路径必须为字符串")?;
        if value.is_empty() || value.contains(['\n', '\r', '\0']) {
            return Err("身份路径含非法字符".into());
        }
        Ok(PathBuf::from(value))
    };
    let command = match value["operation"].as_str() {
        Some("plan") => {
            crate::recovery::clone::native::json::exact(
                &value,
                &[
                    "format_version",
                    "operation",
                    "environment",
                    "output",
                    "write",
                ],
                "身份计划协议",
            )?;
            PerformanceIdentitiesCommand::Plan {
                environment: path("environment")?,
                output: path("output")?,
            }
        }
        Some(operation @ ("apply" | "verify")) => {
            crate::recovery::clone::native::json::exact(
                &value,
                &["format_version", "operation", "plan", "state_dir", "write"],
                "身份执行协议",
            )?;
            if operation == "apply" {
                PerformanceIdentitiesCommand::Apply {
                    plan: path("plan")?,
                    state_dir: path("state_dir")?,
                }
            } else {
                PerformanceIdentitiesCommand::Verify {
                    plan: path("plan")?,
                    state_dir: path("state_dir")?,
                }
            }
        }
        _ => return Err("身份准备阶段无效".into()),
    };
    private_invocation_at(&command, root)?;
    Ok(command)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrivateIdentityInvocation {
    pub(crate) protocol: String,
}

pub(super) fn run(command: &PerformanceIdentitiesCommand) -> Result<()> {
    let root = root_dir();
    if let PerformanceIdentitiesCommand::Plan {
        environment,
        output,
    } = command
    {
        let summary = plan::write(environment, output, &root).map_err(|error| {
            format!("identity_preparation_failed：离线身份计划失败：{error}；请核对环境清单、来源摘要与输出路径；未执行业务写入")
        })?;
        println!("{}", serde_json::to_string(&summary)?);
        return Ok(());
    }
    private_invocation_at(command, &root)?;
    let (path, state_dir) = match command {
        PerformanceIdentitiesCommand::Apply { plan, state_dir } => (plan, state_dir),
        PerformanceIdentitiesCommand::Verify { plan, state_dir } => (plan, state_dir),
        PerformanceIdentitiesCommand::Plan { .. } => unreachable!("离线计划已返回"),
    };
    let summary = execute_at(
        path,
        state_dir,
        command.operation(),
        &uuid::Uuid::new_v4().to_string(),
        None,
        |_| Ok(None),
        &mut |_| Ok(()),
    )?;
    println!("{}", serde_json::to_string(&summary)?);
    Ok(())
}

fn execute_at(
    path: &Path,
    state_dir: &Path,
    mode: &str,
    owner: &str,
    expected_raw_sha256: Option<&str>,
    select_verify_api: impl FnOnce(&Value) -> Result<Option<inspection::VerifiedApi>>,
    checkpoint: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<Value> {
    checkpoint("before-load")?;
    let root = root_dir();
    let bytes = evidence::regular(path)?.ok_or("身份计划文件不存在")?;
    if expected_raw_sha256.is_some_and(|expected| evidence::hash(&bytes) != expected) {
        return Err("身份计划原始字节在授权后变化；未创建账本或远程会话".into());
    }
    let prepared = evidence::parse(&bytes)?;
    let api = {
        let environment = plan::load(&prepared, &root)?;
        if mode == "verify" {
            state::verify_input(&prepared, state_dir, &root)?;
            // 先核对已授权的活动 API，随后才进入会写入 verifying 前缀的执行器。
            select_verify_api(&environment)?
        } else {
            None
        }
    };
    let mut context = native::NativeContext::new(&prepared, state_dir, mode, api)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    checkpoint("before-execute")?;
    let result = runtime.block_on(execution::execute(
        &prepared,
        state_dir,
        mode,
        owner,
        &mut context,
        checkpoint,
    ));
    let value = result?;
    checkpoint("after-execute")?;
    Ok(value)
}

pub(crate) fn private_invocation_at(
    command: &PerformanceIdentitiesCommand,
    root: &Path,
) -> Result<PrivateIdentityInvocation> {
    let protocol = match command {
        PerformanceIdentitiesCommand::Plan {
            environment,
            output,
        } => {
            validate_path(environment, root, LocalTestPathKind::ExistingFile)?;
            validate_path(output, root, LocalTestPathKind::OutputFile)?;
            json!({
                "format_version": 1,
                "operation": "plan",
                "environment": path_text(environment)?,
                "output": path_text(output)?,
                "write": true,
            })
        }
        PerformanceIdentitiesCommand::Apply { plan, state_dir }
        | PerformanceIdentitiesCommand::Verify { plan, state_dir } => {
            validate_path(plan, root, LocalTestPathKind::ExistingFile)?;
            validate_path(state_dir, root, LocalTestPathKind::StateDirectory)?;
            json!({
                "format_version": 1,
                "operation": command.operation(),
                "plan": path_text(plan)?,
                "state_dir": path_text(state_dir)?,
                "write": true,
            })
        }
    };
    Ok(PrivateIdentityInvocation {
        protocol: serialize_protocol(protocol)?,
    })
}

fn validate_path(value: &Path, root: &Path, kind: LocalTestPathKind) -> Result<()> {
    validate_local_test_path(value, root, kind)
        .map(|_| ())
        .map_err(Into::into)
}

fn path_text(value: &Path) -> Result<&str> {
    value
        .to_str()
        .ok_or_else(|| "性能身份路径必须能表示为 UTF-8".into())
}

fn serialize_protocol(protocol: Value) -> Result<String> {
    let serialized = serde_json::to_string(&protocol)?;
    if serialized.len() > MAX_PROTOCOL_BYTES {
        return Err("性能身份私有协议超过 32 KiB 上限".into());
    }
    if serialized.contains(['\n', '\r', '\0']) {
        return Err("性能身份私有协议不能包含换行符或 NUL".into());
    }
    Ok(serialized)
}

#[cfg(test)]
mod tests {
    use super::{MAX_PROTOCOL_BYTES, serialize_protocol};
    use serde_json::Value;

    #[test]
    fn protocol_limit_counts_utf8_bytes_including_json_encoding() {
        let at_limit = Value::String("a".repeat(MAX_PROTOCOL_BYTES - 2));
        assert_eq!(
            serialize_protocol(at_limit).unwrap().len(),
            MAX_PROTOCOL_BYTES
        );
        let oversized = Value::String("密".repeat(MAX_PROTOCOL_BYTES / 3 + 1));
        let error = serialize_protocol(oversized)
            .expect_err("超限协议必须失败")
            .to_string();
        assert!(error.contains("32 KiB"));
        assert!(!error.contains('密'));
    }
}

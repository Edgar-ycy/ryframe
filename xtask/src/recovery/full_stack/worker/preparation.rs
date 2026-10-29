//! monitor 准备失败旁证；不作为候选、就绪或停止授权。
use super::{
    documents,
    operation::{self, Operation},
    role::Role,
};
use crate::{Result, recovery::clone::native::json as strict};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Stage {
    RuntimeInput,
    RequestContract,
    MemberSpawn,
    MemberIdentity,
    ReadyPublication,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Cause {
    PermissionDenied,
    NotFound,
    AddressInUse,
    OtherIo,
    RuntimeChanged,
    RequestMismatch,
    MemberExited,
    Unknown,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Detail {
    stage: Stage,
    cause: Cause,
    os_code: Option<i32>,
    cause_sha256: String,
}

pub(super) fn step<T>(
    op: &Operation,
    monitor: &Value,
    stage: Stage,
    action: impl FnOnce() -> Result<T>,
) -> Result<T> {
    action().inspect_err(|error| {
        // 旁证发布失败不得替代原失败，也不得改变原回收路径。
        let _ = publish(op, monitor, stage, error.as_ref());
    })
}

fn detail(stage: Stage, error: &(dyn std::error::Error + 'static)) -> Detail {
    let mut current = Some(error);
    let mut io = None;
    while let Some(error) = current {
        if let Some(value) = error.downcast_ref::<std::io::Error>()
            && (io.is_none() || value.raw_os_error().is_some())
        {
            io = Some(value);
        }
        current = error.source();
    }
    let text = error.to_string();
    let cause = match io.map(std::io::Error::kind) {
        Some(std::io::ErrorKind::PermissionDenied) => Cause::PermissionDenied,
        Some(std::io::ErrorKind::NotFound) => Cause::NotFound,
        Some(std::io::ErrorKind::AddrInUse) => Cause::AddressInUse,
        Some(_) => Cause::OtherIo,
        None => match text.as_str() {
            "全栈运行二进制与登记摘要不匹配"
            | "业务 fixture 工作树发生漂移"
            | "全栈运行来源证据与当前来源不匹配" => Cause::RuntimeChanged,
            "Worker 控制请求与当前后端、scope、产物或端点不匹配"
            | "角色监督进程不属于当前start操作" => Cause::RequestMismatch,
            "角色监督进程登记前退出" => Cause::MemberExited,
            _ => Cause::Unknown,
        },
    };
    Detail {
        stage,
        cause,
        os_code: io.and_then(std::io::Error::raw_os_error),
        cause_sha256: hex::encode(Sha256::digest(text.as_bytes())),
    }
}

fn publish(
    op: &Operation,
    monitor: &Value,
    stage: Stage,
    error: &(dyn std::error::Error + 'static),
) -> Result<()> {
    if op.role == Role::Frontend || op.request["operation"] != "start" {
        return Ok(());
    }
    op.check()?;
    documents::identity(monitor)?;
    let mut value = op.bound(&op.role.kind("preparation-failed"))?;
    value["role"] = json!(op.role.name());
    value["monitor"] = monitor.clone();
    value["detail"] = serde_json::to_value(detail(stage, error))?;
    op.publish("preparation-failed.json", &value, false)
}

fn bound_detail(
    value: &Value,
    directory: &std::path::Path,
    owner: &Value,
    request: &Value,
) -> Result<Detail> {
    let role = Role::owner(owner)?;
    strict::exact(
        value,
        &[
            "format_version",
            "kind",
            "operation_id",
            "runtime_directory",
            "source_sha256",
            "owner_sha256",
            "request_sha256",
            "role",
            "monitor",
            "detail",
        ],
        "监督准备失败旁证",
    )?;
    documents::common(
        value,
        &role.kind("preparation-failed"),
        &json!(directory),
        &owner["source_sha256"],
    )?;
    documents::bound(value, owner, request)?;
    documents::identity(&value["monitor"])?;
    if role == Role::Frontend
        || request["operation"] != "start"
        || value["role"] != role.name()
        || value["operation_id"] != owner["operation_id"]
    {
        return Err("监督准备失败旁证与原操作不一致".into());
    }
    let detail: Detail =
        serde_json::from_value(value["detail"].clone()).map_err(|_| "监督准备失败旁证类别无效")?;
    validate(&detail)?;
    Ok(detail)
}
fn decode(value: &Value, op: &Operation, monitor: &Value) -> Result<Detail> {
    let detail = bound_detail(value, op.directory(), &op.owner, &op.request)?;
    if value["monitor"] != *monitor {
        return Err("监督准备失败旁证与实际monitor不一致".into());
    }
    Ok(detail)
}

// 来源消费者只复核旁证绑定；不从诊断推导就绪或停止权。
pub(in crate::recovery) fn validate_record(
    value: &Value,
    directory: &std::path::Path,
    owner: &Value,
    request: &Value,
) -> Result<()> {
    bound_detail(value, directory, owner, request).map(|_| ())
}
fn validate(detail: &Detail) -> Result<()> {
    let matching = match detail.cause {
        Cause::RuntimeChanged => matches!(detail.stage, Stage::RuntimeInput),
        Cause::RequestMismatch => matches!(detail.stage, Stage::RequestContract),
        Cause::MemberExited => matches!(detail.stage, Stage::MemberIdentity),
        _ => true,
    };
    if !matching {
        return Err("监督准备阶段与固定原因不一致".into());
    }
    if !documents::hex(&json!(detail.cause_sha256), 64)
        || (detail.os_code.is_some()
            && !matches!(
                detail.cause,
                Cause::PermissionDenied | Cause::NotFound | Cause::AddressInUse | Cause::OtherIo
            ))
    {
        return Err("监督准备失败旁证系统码或摘要无效".into());
    }
    Ok(())
}
fn advice(stage: Stage) -> &'static str {
    match stage {
        Stage::RuntimeInput => "核对冻结来源、配置与产物；保留现场，显式核对，不重放",
        Stage::RequestContract => "核对原owner/request与运行合同；保留现场，显式核对，不重放",
        Stage::MemberSpawn => "核对成员创建权限与系统资源；保留现场，显式核对，不重放",
        Stage::MemberIdentity => "核对成员创建身份与退出证据；保留现场，显式核对，不重放",
        Stage::ReadyPublication => "核对成员ready证据目录与权限；保留现场，显式核对，不重放",
    }
}
fn message(role: Role, detail: &Detail) -> String {
    let stage = serde_json::to_value(detail.stage).expect("固定阶段");
    let cause = serde_json::to_value(detail.cause).expect("固定原因");
    format!(
        "{}监督准备失败；阶段={}；原因={}；系统码={}；原因摘要={}；{}",
        role.label(),
        stage.as_str().expect("固定阶段字符串"),
        cause.as_str().expect("固定原因字符串"),
        detail
            .os_code
            .map_or_else(|| "none".into(), |code| code.to_string()),
        detail.cause_sha256,
        advice(detail.stage)
    )
}

pub(super) fn failure(op: &Operation, monitor: &Value) -> Option<String> {
    let read = || -> Result<String> {
        op.check()?;
        let value = operation::read(&op.path.join("preparation-failed.json"))?;
        let detail = decode(&value, op, monitor)?;
        op.check()?;
        Ok(message(op.role, &detail))
    };
    read().ok()
}

// 外层交接只公开严格重建的固定诊断；未知文本仍只报告有界长度和摘要。
pub(in crate::recovery) fn safe_message(role: Role, text: &str) -> bool {
    if role == Role::Frontend {
        return false;
    }
    let fields = text.split('；').collect::<Vec<_>>();
    if fields.len() != 7 {
        return false;
    }
    let parse = || -> Option<Detail> {
        Some(Detail {
            stage: serde_json::from_value(json!(fields[1].strip_prefix("阶段=")?)).ok()?,
            cause: serde_json::from_value(json!(fields[2].strip_prefix("原因=")?)).ok()?,
            os_code: match fields[3].strip_prefix("系统码=")? {
                "none" => None,
                code => Some(code.parse().ok()?),
            },
            cause_sha256: fields[4].strip_prefix("原因摘要=")?.into(),
        })
    };
    parse().is_some_and(|detail| validate(&detail).is_ok() && message(role, &detail) == text)
}

#[cfg(test)]
#[path = "../../../../tests/unit/recovery/full_stack/worker/preparation_tests.rs"]
mod tests;

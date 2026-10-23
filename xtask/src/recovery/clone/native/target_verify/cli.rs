//! fresh-target 私有 CLI 的参数边界与本地状态投影，不执行远端写入。
use std::path::Path;

use serde_json::{Value, json};

use super::{
    super::json as json_io,
    evidence_model::{binding, bound, check, read},
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

mod status;
#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/target_verify/cli/tests.rs"]
mod tests;

const DIAGNOSTICS: &[&str] = &[
    "fresh-target 参数必须严格匹配当前阶段；请核对当前操作的请求、环境、存储和观察目录参数",
    "fresh-target status 是只读操作，不接受 --write",
    "fresh-target 非 status 操作需要显式 --write",
    "fresh-target 操作无效；请使用 prepare、resume-prepare、initialize、resume-initialize、reconcile-preflight、verify 或 status",
    "fresh 目标登记类型无效；请核对 registration.json，禁止重放初始化",
    "fresh 目标目录必须固定在登记 workspace",
    "fresh 目标登记在读取期间变化；请保留现场并核对来源",
    "fresh 目标 workspace、storage run 与观察目录必须互不包含",
    "fresh 目标登记后仅允许已收尾的受控目标存储阶段",
    "fresh 目标状态观察无效；请核对原始收据，不自动重放或清理",
];

pub(super) fn diagnostic(reason: &str) -> Option<&'static str> {
    DIAGNOSTICS
        .iter()
        .copied()
        .find(|message| *message == reason)
        .or_else(|| super::creation_plan::diagnostic(reason))
}

pub(super) fn execute(
    backend: &Path,
    target: &Path,
    operation: &str,
    args: &Value,
) -> Result<Value> {
    match operation {
        "cli-dispatch" => dispatch(args),
        "cli-registration" => {
            json_io::exact(args, &[], "目标登记参数")?;
            registration(backend, target)
        }
        "cli-disjoint" => {
            json_io::exact(args, &["paths"], "目标目录参数")?;
            disjoint(
                &json_io::array(&args["paths"], "目标目录")?
                    .iter()
                    .map(|value| Ok(Path::new(json_io::text(value, "目标目录")?)))
                    .collect::<Result<Vec<_>>>()?,
            )?;
            Ok(json!({"status":"disjoint_verified"}))
        }
        "cli-storage-append" => storage_append(args),
        "cli-status" => status::project(args),
        "prepare-resume-state" => {
            json_io::exact(args, &["request_descriptor"], "prepare 状态参数")?;
            super::prepare_state::state(backend, target, &args["request_descriptor"])
        }
        "prepare-failure" => {
            json_io::exact(args, &[], "prepare 失败参数")?;
            // 失败详情留在原文件；不通过标准输出复制潜在凭据。
            Ok(json!({"present":super::prepare_state::failure(backend,target)?.is_some()}))
        }
        "prepare-failure-resolved" => {
            json_io::exact(args, &["request_descriptor"], "prepare 失败收尾参数")?;
            Ok(
                json!({"resolved":super::prepare_state::resolved(backend,target,&args["request_descriptor"])}),
            )
        }
        _ => super::creation_plan::execute(backend, target, operation, args),
    }
}

fn dispatch(args: &Value) -> Result<Value> {
    json_io::exact(
        args,
        &["operation", "write", "supplied"],
        "fresh-target 参数",
    )?;
    let operation = json_io::text(&args["operation"], "fresh-target 操作")?;
    let expected = match operation {
        "prepare" => [true, true, true, false],
        "verify" => [false, false, false, true],
        "status"
        | "resume-prepare"
        | "initialize"
        | "resume-initialize"
        | "reconcile-preflight" => [false; 4],
        _ => return Err(DIAGNOSTICS[3].into()),
    };
    check(args["supplied"] == json!(expected), DIAGNOSTICS[0])?;
    let write = args["write"].as_bool().ok_or(DIAGNOSTICS[0])?;
    check(operation != "status" || !write, DIAGNOSTICS[1])?;
    check(operation == "status" || write, DIAGNOSTICS[2])?;
    Ok(json!({"operation":operation,"remote_writes":0,"automatic_retry":false}))
}

fn registration(backend: &Path, workspace: &Path) -> Result<Value> {
    let path = workspace.join("registration.json");
    let before = binding(backend, &path)?;
    let value = read(backend, &path)?;
    json_io::exact(
        &value,
        &[
            "format_version",
            "kind",
            "request",
            "environment",
            "storage_run",
            "target_directory",
        ],
        "目标登记",
    )?;
    check(
        value["format_version"].as_u64() == Some(1)
            && value["kind"] == "devex-clone-fresh-target-registration",
        DIAGNOSTICS[4],
    )?;
    check(
        Path::new(json_io::text(&value["target_directory"], "目标目录")?)
            == workspace.join("target"),
        DIAGNOSTICS[5],
    )?;
    bound(backend, &value["request"])?;
    bound(backend, &value["environment"])?;
    let storage = &value["storage_run"];
    json_io::exact(storage, &["path", "manifest", "state"], "目标存储绑定")?;
    let directory = Path::new(json_io::text(&storage["path"], "目标存储目录")?);
    validate_local_test_path(directory, backend, LocalTestPathKind::ExistingDirectory)?;
    check(
        bound(backend, &storage["manifest"])? == directory.join("manifest.json"),
        DIAGNOSTICS[4],
    )?;
    disjoint(&[workspace, directory])?;
    check(binding(backend, &path)? == before, DIAGNOSTICS[6])?;
    Ok(json!({"path":path,"value":value,"binding":before}))
}

fn disjoint(paths: &[&Path]) -> Result<()> {
    for (index, path) in paths.iter().enumerate() {
        check(path.is_absolute(), DIAGNOSTICS[7])?;
        for other in &paths[index + 1..] {
            check(
                !path.starts_with(other) && !other.starts_with(path),
                DIAGNOSTICS[7],
            )?;
        }
    }
    Ok(())
}

fn storage_append(args: &Value) -> Result<Value> {
    json_io::exact(args, &["attempts"], "目标存储追加参数")?;
    let mut storage = false;
    let mut cache = false;
    for attempt in json_io::array(&args["attempts"], "目标存储追加阶段")? {
        let allowed = match attempt["stage"].as_str() {
            Some("storage-target") => {
                storage = true;
                &["restart", "stop", "recover"][..]
            }
            Some("cache-target") => {
                cache = true;
                &["restart", "stop", "recover", "reconcile", "resume"][..]
            }
            _ => return Err(DIAGNOSTICS[8].into()),
        };
        check(
            attempt["mode"]
                .as_str()
                .is_some_and(|mode| allowed.contains(&mode))
                && attempt["status"]
                    .as_str()
                    .is_some_and(|status| status != "running"),
            DIAGNOSTICS[8],
        )?;
    }
    Ok(json!({"storage":storage,"cache":cache}))
}

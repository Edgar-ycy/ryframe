//! 从本次受管启动记录绑定原 API 进程，拒绝端口上的其他代次。
use std::{ffi::OsString, path::Path};

use serde_json::{Value, json};

use super::Options;
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path, validate_local_test_path},
    recovery::{
        clone::native::json as strict,
        fixture_control::services::lifecycle::readonly::process,
        runtime::{dataset_authority::commands::same_path, generation::receipts},
    },
};

pub(super) fn bind(options: &Options, arguments: &[OsString]) -> Result<Value> {
    let root = validate_absolute_path(
        &options.runtime_directory,
        LocalTestPathKind::ExistingDirectory,
    )?;
    let state_file = validate_absolute_path(
        &root.join("lifecycle.json"),
        LocalTestPathKind::ExistingFile,
    )?;
    let state = strict::read(&state_file)?;
    let (generation, directory) = active(&state, &root)?;
    let request = &generation["request"];
    let coordinator = recorded_path(&request["tools"]["coordinator_root"])?;
    validate_local_test_path(&root, coordinator, LocalTestPathKind::ExistingDirectory)?;
    check(
        request["format_version"].as_u64() == Some(2)
            && request["kind"] == "restore-runtime-launch-request"
            && request["registration"] == state["registration"]
            && same_path(&root, recorded_path(&request["runtime_directory"])?)?,
        "前端代理启动记录未绑定同一恢复登记",
    )?;
    command(generation, arguments)?;
    let authority = &request["authority"];
    let api_origin = format!("http://127.0.0.1:{}", options.api_port);
    check(
        authority["api_endpoint"] == format!("{api_origin}/readyz")
            && authority["frontend_endpoint"] == format!("http://{}", options.address)
            && authority["frontend_sha"] == options.expected_head
            && request["digests"]["frontend_build"] == options.receipt_sha256
            && same_path(
                &options.receipt,
                recorded_path(&request["paths"]["frontend_build"])?,
            )?
            && same_path(
                &options.frontend,
                recorded_path(&request["roots"]["frontend"])?,
            )?,
        "前端代理端点或产物未绑定同一启动代次",
    )?;
    api(generation, &directory, &authority["scope_id"])
}

fn active<'a>(state: &'a Value, root: &Path) -> Result<(&'a Value, std::path::PathBuf)> {
    strict::exact(
        state,
        &[
            "format_version",
            "kind",
            "runtime_directory",
            "registration",
            "creation_intent",
            "generations",
        ],
        "前端代理运行代次",
    )?;
    let generations = strict::array(&state["generations"], "前端代理运行代次列表")?;
    let generation = generations.last().ok_or("前端代理缺少已登记的启动代次")?;
    let directory = root.join(format!("generation-{:04}", generations.len()));
    check(
        state["format_version"].as_u64() == Some(1)
            && state["kind"] == "restore-runtime-lifecycle"
            && same_path(root, recorded_path(&state["runtime_directory"])?)?
            && generation["number"].as_u64() == Some(generations.len().try_into()?)
            && generation["status"] == "starting"
            && generation["roles"]["api"]["state"] == "running"
            && same_path(&directory, recorded_path(&generation["directory"])?)?,
        "前端代理仅接受本次启动中的运行代次和已就绪的 API",
    )?;
    let directory = validate_absolute_path(&directory, LocalTestPathKind::ExistingDirectory)?;
    Ok((generation, directory))
}

fn command(generation: &Value, arguments: &[OsString]) -> Result<()> {
    let frontend = &generation["roles"]["frontend"];
    let recorded = strict::array(&frontend["command"], "前端代理登记命令")?;
    let executable = std::env::current_exe()?;
    check(
        matches!(frontend["state"].as_str(), Some("pending" | "running"))
            && recorded.len() == arguments.len() + 1
            && same_path(&executable, recorded_path(&recorded[0])?)?
            && recorded[1..]
                .iter()
                .zip(arguments)
                .all(|(expected, actual)| expected.as_str() == actual.to_str()),
        "当前前端代理命令与该代次的登记命令不同",
    )
}

fn api(generation: &Value, directory: &Path, scope: &Value) -> Result<Value> {
    let role = &generation["roles"]["api"];
    let request = &generation["request"];
    let tree = receipts::tree(directory, "api", scope)?;
    let (identity, _) = receipts::process(directory, "api", scope)?;
    check(
        tree == role["tree"]
            && tree["operation_id"] == role["operation_id"]
            && tree["process"] == identity
            && same_path(
                recorded_path(&request["artifacts"]["api"]["executable"])?,
                recorded_path(&identity["executable"])?,
            )?
            && role["command"]
                == json!([
                    request["artifacts"]["api"]["executable"],
                    "--config-dir",
                    request["environment"]["configurations"]["api"]["directory"]
                ]),
        "前端代理的 API 创建身份、命令或进程树与同代记录不同",
    )?;
    for member in ["supervisor", "monitor", "process"] {
        process::require(&tree[member])?;
    }
    Ok(identity)
}

fn recorded_path(value: &Value) -> Result<&Path> {
    Ok(Path::new(strict::text(value, "前端代理登记路径")?))
}

fn check(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

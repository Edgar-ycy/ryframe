//! 现有恢复生命周期的代次模型与收据发布；不启动、接管或重放进程。
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{Result, recovery::clone::native::json as strict};

mod files;
pub(in crate::recovery::runtime) mod receipts;
#[cfg(test)]
#[path = "../../../../tests/unit/recovery/runtime/generation/tests.rs"]
mod tests;

pub(super) use files::generation_directory as verify_directory;
pub(super) use files::receipt_present;

pub(super) const ROLES: [&str; 3] = ["api", "worker", "frontend"];
pub(super) const STATE: &str = "lifecycle.json";
pub(super) const LAUNCH: &str = "runtime-launch.json";

pub(crate) fn writes(args: &Value) -> bool {
    matches!(
        args["action"].as_str(),
        Some("create-intent" | "save-state" | "write-launch")
    )
}

pub(crate) fn request(args: &Value) -> Result<Value> {
    strict::exact(
        args,
        &[
            "action", "root", "binding", "value", "previous", "commands", "number",
        ],
        "恢复代次请求",
    )?;
    let root = path(&args["root"])?;
    binding(&args["binding"])?;
    let value = &args["value"];
    let commands = &args["commands"];
    match text(&args["action"])? {
        "new-generation" => new_generation(&root, integer(&args["number"])?, value, commands),
        "initial-state" => {
            let creation = files::document(&creation_path(&root))?;
            validate_creation(&creation, &root, &args["binding"])?;
            files::bound(&creation_path(&root), &args["previous"])?;
            check(
                creation["request"] == *value,
                "首代运行请求与首次 intent 不一致",
            )?;
            Ok(
                json!({"format_version":1,"kind":"restore-runtime-lifecycle","runtime_directory":root,
                "registration":args["binding"],"creation_intent":args["previous"],
                "generations":[new_generation(&root,1,value,commands)?]}),
            )
        }
        "validate-generation" => {
            validate_generation(
                value,
                &root,
                integer(&args["number"])?,
                &args["binding"],
                commands,
            )?;
            Ok(value.clone())
        }
        "validate-creation" => {
            validate_creation(value, &root, &args["binding"])?;
            Ok(value.clone())
        }
        "create-intent" => create_intent(&root, &args["binding"], value),
        "read-creation" => {
            let value = files::document(&creation_path(&root))?;
            validate_creation(&value, &root, &args["binding"])?;
            if !args["previous"].is_null() {
                files::bound(&creation_path(&root), &args["previous"])?;
            }
            Ok(value)
        }
        "validate-state" => {
            validate_state(value, &root, &args["binding"], commands)?;
            Ok(value.clone())
        }
        "save-state" => save_state(&root, value, &args["binding"], &args["previous"], commands),
        "validate-launch" => {
            receipts::validate_launch(value, &root, commands)?;
            Ok(value.clone())
        }
        "write-launch" => receipts::write_launch(&root, value, commands),
        _ => Err("未知恢复代次模型操作".into()),
    }
}

pub(super) fn check(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}
pub(super) fn text(value: &Value) -> Result<&str> {
    strict::text(value, "恢复代次字符串")
}
pub(super) fn integer(value: &Value) -> Result<u64> {
    value
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or_else(|| "恢复代次编号必须为正整数".into())
}
pub(super) fn path(value: &Value) -> Result<PathBuf> {
    let path = PathBuf::from(text(value)?);
    files::safe(&path)?;
    Ok(path)
}
pub(super) fn hex(value: &Value, length: usize) -> bool {
    value.as_str().is_some_and(|s| {
        s.len() == length
            && s.bytes()
                .all(|c| c.is_ascii_digit() || matches!(c, b'a'..=b'f'))
    })
}
pub(super) fn descriptor(value: &Value) -> Result<()> {
    strict::exact(value, &["path", "bytes", "sha256"], "恢复文件描述")?;
    path(&value["path"])?;
    integer(&value["bytes"])?;
    check(hex(&value["sha256"], 64), "恢复文件描述摘要无效")
}
fn binding(value: &Value) -> Result<()> {
    strict::exact(value, &["registration", "target_plan"], "恢复登记绑定")?;
    descriptor(&value["registration"])?;
    descriptor(&value["target_plan"])
}

pub(super) fn creation_path(root: &Path) -> PathBuf {
    root.with_file_name(format!(
        "{}.runtime-create.json",
        root.file_name().unwrap_or_default().to_string_lossy()
    ))
}
fn validate_creation(value: &Value, root: &Path, binding: &Value) -> Result<()> {
    strict::exact(
        value,
        &[
            "format_version",
            "kind",
            "runtime_directory",
            "generation",
            "registration",
            "request",
        ],
        "恢复运行首次目录 intent",
    )?;
    check(
        value["format_version"].as_u64() == Some(1)
            && value["kind"] == "restore-runtime-create-intent"
            && value["runtime_directory"] == json!(root)
            && value["generation"].as_u64() == Some(1)
            && value["registration"] == *binding
            && value["request"]["registration"] == *binding
            && value["request"]["runtime_directory"] == json!(root),
        "恢复运行首次目录 intent 与登记或目录不匹配",
    )
}
fn create_intent(root: &Path, binding: &Value, request: &Value) -> Result<Value> {
    files::write_scope(root, request)?;
    let value = json!({"format_version":1,"kind":"restore-runtime-create-intent",
        "runtime_directory":root,"generation":1,"registration":binding,"request":request});
    validate_creation(&value, root, binding)?;
    let output = creation_path(root);
    strict::write_new(&output, &value)?;
    files::bound_value(&output)
}
fn new_generation(root: &Path, number: u64, request: &Value, commands: &Value) -> Result<Value> {
    check(
        request["runtime_directory"] == json!(root),
        "恢复启动请求必须绑定本次运行目录",
    )?;
    receipts::commands(commands)?;
    let directory = root.join(format!("generation-{number:04}"));
    let mut roles = serde_json::Map::new();
    for role in ROLES {
        roles.insert(role.to_owned(), json!({"state":"pending","operation_id":uuid::Uuid::new_v4().simple().to_string(),
            "command":commands[role],"log":directory.join(format!("{role}.log")),"tree":null,"completion":null}));
    }
    Ok(
        json!({"number":number,"directory":directory,"status":"starting","request":request,
        "roles":roles,"launch":null,"error_type":null}),
    )
}

fn validate_generation(
    value: &Value,
    root: &Path,
    number: u64,
    binding: &Value,
    commands: &Value,
) -> Result<()> {
    strict::exact(
        value,
        &[
            "number",
            "directory",
            "status",
            "request",
            "roles",
            "launch",
            "error_type",
        ],
        "恢复运行代次",
    )?;
    let directory = root.join(format!("generation-{number:04}"));
    check(
        value["number"].as_u64() == Some(number)
            && value["directory"] == json!(directory)
            && matches!(
                value["status"].as_str(),
                Some(
                    "starting"
                        | "running"
                        | "stopping"
                        | "stop_failed"
                        | "cleanup_failed"
                        | "stopped"
                        | "start_failed"
                )
            )
            && value["request"]["registration"] == *binding
            && value["request"]["runtime_directory"] == json!(root)
            && (value["error_type"].is_null() || value["error_type"].is_string()),
        "恢复运行代次编号、状态或登记绑定无效",
    )?;
    receipts::commands(commands)?;
    strict::exact(&value["roles"], &ROLES, "恢复运行角色")?;
    let mut operations = std::collections::BTreeSet::new();
    for role in ROLES {
        let item = &value["roles"][role];
        strict::exact(
            item,
            &[
                "state",
                "operation_id",
                "command",
                "log",
                "tree",
                "completion",
            ],
            "恢复运行角色状态",
        )?;
        check(
            matches!(
                item["state"].as_str(),
                Some("pending" | "running" | "stopped")
            ) && hex(&item["operation_id"], 32)
                && operations.insert(text(&item["operation_id"])?)
                && item["command"] == commands[role]
                && item["log"] == json!(directory.join(format!("{role}.log")))
                && (item["tree"].is_null() || item["tree"].is_object())
                && (item["completion"].is_null() || item["completion"].is_object()),
            "恢复运行角色状态无效",
        )?;
    }
    files::generation_directory(&directory, &value["roles"], &value["request"])?;
    receipts::generation_receipts(value, &directory, commands)
}

fn validate_state(value: &Value, root: &Path, binding: &Value, commands: &Value) -> Result<()> {
    strict::exact(
        value,
        &[
            "format_version",
            "kind",
            "runtime_directory",
            "registration",
            "creation_intent",
            "generations",
        ],
        "恢复运行生命周期",
    )?;
    let generations = strict::array(&value["generations"], "恢复运行代次集合")?;
    let expected_commands = strict::array(commands, "恢复运行命令集合")?;
    check(
        value["format_version"].as_u64() == Some(1)
            && value["kind"] == "restore-runtime-lifecycle"
            && value["runtime_directory"] == json!(root)
            && value["registration"] == *binding
            && !generations.is_empty()
            && generations.len() == expected_commands.len(),
        "恢复运行生命周期与当前目录或登记不匹配",
    )?;
    let creation = files::document(&creation_path(root))?;
    validate_creation(&creation, root, binding)?;
    files::bound(&creation_path(root), &value["creation_intent"])?;
    for (index, generation) in generations.iter().enumerate() {
        validate_generation(
            generation,
            root,
            index as u64 + 1,
            binding,
            &expected_commands[index],
        )?;
    }
    let mut expected = std::collections::BTreeSet::from([STATE.to_owned()]);
    for (index, generation) in generations.iter().enumerate() {
        let missing_latest =
            index == generations.len() - 1 && missing_latest_directory(generation)?;
        let name = format!("generation-{:04}", index + 1);
        if !missing_latest || root.join(&name).try_exists()? {
            expected.insert(name);
        }
    }
    check(
        fs::symlink_metadata(root)?.is_dir(),
        "恢复运行根目录不是普通目录",
    )?;
    let actual = files::names(root)?;
    check(
        actual == expected,
        "恢复运行目录包含缺失、额外或未知的生命周期文件",
    )
}
fn missing_latest_directory(value: &Value) -> Result<bool> {
    let roles = strict::object(&value["roles"], "恢复运行角色")?;
    Ok(value["status"] == "starting"
        && value["launch"].is_null()
        && value["error_type"].is_null()
        && roles
            .values()
            .all(|item| item["state"] == "pending" && item["tree"].is_null()))
}
fn save_state(
    root: &Path,
    value: &Value,
    binding: &Value,
    previous: &Value,
    commands: &Value,
) -> Result<Value> {
    let generations = strict::array(&value["generations"], "恢复运行代次")?;
    let last = generations.last().ok_or("恢复运行缺少代次")?;
    files::write_scope(root, &last["request"])?;
    let output = root.join(STATE);
    if previous.is_null() {
        check(!output.try_exists()?, "恢复运行生命周期在创建前被替换")?;
        strict::write_new(&output, value)?;
    } else {
        files::bound(&output, previous)?;
        strict::write_replace(&output, value)?;
    }
    validate_state(value, root, binding, commands)?;
    files::bound_value(&output)
}

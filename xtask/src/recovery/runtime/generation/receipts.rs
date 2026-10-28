use super::{LAUNCH, ROLES, check, descriptor, files, hex, integer, text};
use crate::{
    Result,
    recovery::clone::native::{json as strict, resources::common},
};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn commands(value: &Value) -> Result<()> {
    strict::exact(value, &ROLES, "恢复运行角色命令")?;
    for role in ROLES {
        let command = strict::array(&value[role], "恢复运行命令")?;
        check(
            !command.is_empty()
                && command
                    .iter()
                    .all(|arg| arg.as_str().is_some_and(|s| !s.contains('\0'))),
            "恢复运行命令必须是非空字符串参数集合",
        )?;
        super::path(&command[0])?;
    }
    Ok(())
}
fn tree(directory: &Path, role: &str, scope: &Value) -> Result<Value> {
    let value = files::document(&directory.join(format!("{role}-tree.json")))?;
    strict::exact(
        &value,
        &[
            "format_version",
            "kind",
            "runtime_directory",
            "role",
            "scope_id",
            "operation_id",
            "supervisor",
            "process",
            "monitor",
            "group_id",
        ],
        "恢复完整进程树",
    )?;
    check(
        value["format_version"].as_u64() == Some(2)
            && value["kind"] == "full-stack-process-tree"
            && value["runtime_directory"] == json!(directory)
            && value["role"] == role
            && value["scope_id"] == *scope
            && hex(&value["operation_id"], 32),
        "恢复完整进程树与代次、角色或scope不匹配",
    )?;
    for name in ["supervisor", "process", "monitor"] {
        common::validate_process_identity(&value[name])?;
    }
    check(
        value["group_id"] == value["supervisor"]["pid"]
            && value["monitor"]["pid"] != value["supervisor"]["pid"]
            && value["monitor"]["pid"] != value["process"]["pid"],
        "恢复进程树缺少可信进程组或树外监督器",
    )?;
    Ok(value)
}
fn process(directory: &Path, role: &str, scope: &Value) -> Result<(Value, Value)> {
    let path = directory.join(format!("{role}.json"));
    let value = files::document(&path)?;
    strict::exact(
        &value,
        &["format_version", "role", "scope_id", "identity"],
        "恢复产品进程收据",
    )?;
    // 进程收据与现有监督协议保持同构，只提取权威创建身份。
    check(
        value["format_version"].as_u64() == Some(1)
            && value["role"] == role
            && value["scope_id"] == *scope,
        "恢复产品进程收据与角色或scope不匹配",
    )?;
    let identity = common::validate_process_identity(&value["identity"])?;
    Ok((identity, files::bound_value(&path)?))
}
fn launch_binding(directory: &Path, value: &Value, commands: &Value) -> Result<()> {
    descriptor(value)?;
    let output = directory.join(LAUNCH);
    files::bound(&output, value)?;
    validate_launch(&files::document(&output)?, directory, commands)
}
pub(super) fn generation_receipts(value: &Value, directory: &Path, commands: &Value) -> Result<()> {
    let launch = &value["launch"];
    if !launch.is_null() {
        launch_binding(directory, launch, commands)?;
    } else if directory.join(LAUNCH).try_exists()? {
        let interrupted = value["status"] == "starting"
            && value["error_type"].is_null()
            && ROLES.iter().all(|r| {
                value["roles"][r]["state"] == "running" && !value["roles"][r]["tree"].is_null()
            });
        check(interrupted, "恢复运行代次存在未登记的启动成功收据")?;
        validate_launch(
            &files::document(&directory.join(LAUNCH))?,
            directory,
            commands,
        )?;
    }
    let scope = &value["request"]["authority"]["scope_id"];
    for role in ROLES {
        let item = &value["roles"][role];
        let process_path = directory.join(format!("{role}.json"));
        let tree_path = directory.join(format!("{role}-tree.json"));
        check(
            !process_path.try_exists()? || tree_path.try_exists()?,
            "恢复产品进程收据缺少完整树证据",
        )?;
        if !item["tree"].is_null() {
            let current = tree(directory, role, scope)?;
            let (identity, _) = process(directory, role, scope)?;
            check(
                current == item["tree"]
                    && current["operation_id"] == item["operation_id"]
                    && current["process"] == identity
                    && directory.join(format!("{role}.log")).is_file(),
                "恢复运行状态没有绑定当前进程树文件",
            )?;
        }
        if !item["completion"].is_null() {
            let name = format!(
                "{role}-members-{}-stopped.json",
                text(&item["operation_id"])?
            );
            files::document(&directory.join(&name))?;
            files::bound(&directory.join(name), &item["completion"])?;
        }
    }
    check(
        value["status"] != "running" || !launch.is_null(),
        "运行代次缺少成功启动收据",
    )
}
pub(super) fn validate_launch(
    value: &Value,
    directory: &Path,
    expected_commands: &Value,
) -> Result<()> {
    strict::exact(
        value,
        &[
            "format_version",
            "kind",
            "generation",
            "runtime_directory",
            "request",
            "processes",
        ],
        "恢复运行启动收据",
    )?;
    check(
        value["format_version"].as_u64() == Some(1)
            && value["kind"] == "restore-runtime-launch"
            && value["runtime_directory"] == json!(directory),
        "恢复运行启动收据版本、代次或目录不匹配",
    )?;
    integer(&value["generation"])?;
    commands(expected_commands)?;
    strict::exact(&value["processes"], &ROLES, "恢复启动进程")?;
    for role in ROLES {
        validate_launch_role(value, directory, expected_commands, role)?;
    }
    Ok(())
}
fn validate_launch_role(
    value: &Value,
    directory: &Path,
    commands: &Value,
    role: &str,
) -> Result<()> {
    let item = &value["processes"][role];
    strict::exact(
        item,
        &["tree", "command", "log", "process_receipt"],
        "恢复启动进程",
    )?;
    strict::exact(
        &item["process_receipt"],
        &["path", "bytes", "sha256", "identity"],
        "恢复进程收据",
    )?;
    let (identity, mut binding) =
        process(directory, role, &value["request"]["authority"]["scope_id"])?;
    binding["identity"] = identity.clone();
    let current_tree = tree(directory, role, &value["request"]["authority"]["scope_id"])?;
    check(
        item["command"] == commands[role]
            && item["log"] == json!(directory.join(format!("{role}.log")))
            && item["process_receipt"] == binding
            && item["tree"] == current_tree
            && current_tree["process"] == identity
            && crate::recovery::runtime::dataset_authority::commands::same_path(
                &super::path(&identity["executable"])?,
                &super::path(&commands[role][0])?,
            )?,
        "恢复启动收据没有绑定当前命令和完整进程树",
    )
}
pub(super) fn write_launch(
    directory: &Path,
    generation: &Value,
    commands: &Value,
) -> Result<Value> {
    let root = directory.parent().ok_or("恢复代次目录缺少父目录")?;
    files::write_scope(root, &generation["request"])?;
    check(
        generation["directory"] == json!(directory),
        "恢复启动收据输出目录不匹配",
    )?;
    let mut processes = serde_json::Map::new();
    for role in ROLES {
        let item = &generation["roles"][role];
        let (identity, mut binding) = process(
            directory,
            role,
            &generation["request"]["authority"]["scope_id"],
        )?;
        binding["identity"] = identity;
        processes.insert(role.to_owned(), json!({"tree":item["tree"],"command":item["command"],"log":item["log"],"process_receipt":binding}));
    }
    let value = json!({"format_version":1,"kind":"restore-runtime-launch","generation":generation["number"],
        "runtime_directory":directory,"request":generation["request"],"processes":processes});
    validate_launch(&value, directory, commands)?;
    let output = directory.join(LAUNCH);
    strict::write_new(&output, &value)?;
    files::bound_value(&output)
}

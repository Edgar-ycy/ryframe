//! 原生角色目录只消费同一意图的完整控制链，不以名称白名单代替授权。
use super::*;
use crate::recovery::{fixture_control, full_stack::worker};

#[path = "control/members.rs"]
mod members;

pub(super) fn verify(runtime: &Path, role: &str, id: &str) -> Result<()> {
    let lock = runtime.join(format!("{role}-control.lock"));
    let archives = runtime.join(format!("{role}-control-receipts"));
    let guard = runtime.join(format!("{role}-control.guard"));
    if guard.try_exists()? {
        plain(&guard, false)?;
        crate::recovery::runtime::dataset_authority::reads::file(&guard)?;
        if std::fs::read(&guard)? != [0] {
            return Err("源角色内核互斥文件内容无效".into());
        }
    }
    let mut selected = None;
    if lock.try_exists()? {
        selected = Some((lock.clone(), false));
    }
    if archives.try_exists()? {
        plain(&archives, true)?;
        let entries = crate::recovery::runtime::dataset_authority::reads::entries(&archives)?
            .collect::<std::io::Result<Vec<_>>>()?;
        if entries.len() != 1 || entries[0].file_name().to_str() != Some(id) || selected.is_some() {
            return Err("源角色档案必须恰为同一启动ID且不能与运行锁并存".into());
        }
        selected = Some((entries[0].path(), true));
    }
    let Some((directory, archived)) = selected else {
        let names = [
            format!("{role}-tree.json"),
            format!("{role}.json"),
            format!("{role}-{id}.log"),
            format!("{role}-members-{id}-ready.json"),
            format!("{role}-members-{id}-stopped.json"),
            format!("{role}-tree-{id}-control.json"),
            format!("{role}-tree-{id}-result.json"),
        ];
        if names.iter().any(|name| runtime.join(name).exists()) {
            return Err("源角色进程缺少原生启动owner/request链；拒绝接管或补签".into());
        }
        return Ok(());
    };
    if !guard.try_exists()? {
        return Err("源角色控制收据缺少原内核互斥文件".into());
    }
    receipt(runtime, role, id, &directory, archived)
}

fn plain(path: &Path, directory: bool) -> Result<()> {
    crate::local_test_path::validate_absolute_path(
        path,
        if directory {
            crate::local_test_path::LocalTestPathKind::ExistingDirectory
        } else {
            crate::local_test_path::LocalTestPathKind::ExistingFile
        },
    )?;
    let metadata = std::fs::symlink_metadata(path)?;
    if digest::linked(&metadata) || (!directory && common::link_count_for(path, &metadata)? != 1) {
        return Err("源角色证据不能经过链接或硬链接".into());
    }
    Ok(())
}

fn read(path: &Path) -> Result<Value> {
    plain(path, false)?;
    let before = common::binding(path)?;
    if !before["bytes"]
        .as_u64()
        .is_some_and(|n| n > 0 && n <= 4 * 1024 * 1024)
    {
        return Err("源角色控制收据超过既有4MiB上限或为空".into());
    }
    let value = document::read(path)?;
    if common::binding(path)? != before {
        return Err("源角色控制收据在读取期间变化".into());
    }
    Ok(value)
}

fn receipt(runtime: &Path, role: &str, id: &str, directory: &Path, archived: bool) -> Result<()> {
    plain(directory, true)?;
    let allowed = [
        "owner.json",
        "request.json",
        "candidate.json",
        "supervisor.json",
        "progress.json",
        "result.json",
    ];
    let mut records = serde_json::Map::new();
    let mut bindings = Vec::new();
    for entry in crate::recovery::runtime::dataset_authority::reads::entries(directory)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "源角色收据名称必须为UTF-8")?;
        if !allowed.contains(&name.as_str()) {
            return Err("源角色控制目录存在未知证据".into());
        }
        plain(&entry.path(), false)?;
        bindings.push(common::binding(&entry.path())?);
        records.insert(name, read(&entry.path())?);
    }
    let owner = records.get("owner.json").ok_or("源角色缺少owner")?;
    let request = records.get("request.json").ok_or("源角色缺少request")?;
    if owner["kind"] != format!("full-stack-{role}-control-owner")
        || owner["operation_id"] != id
        || request["operation"] != "start"
    {
        return Err("源角色控制链不属于原启动角色与ID".into());
    }
    let mut contract = inputs(runtime, role, request)?;
    contract["api_ready_url"] = request["api_ready_url"].clone();
    let source = digest::plan_hash(
        &json!({"runtime":file(&runtime.join("runtime.json"))?,"runtime_evidence":null}),
    )?;
    let mut args = json!({"mode":"request","directory":runtime,"source":source,"owner":owner,"request":request,
        "value":null,"candidate":records.get("candidate.json"),"supervisor":records.get("supervisor.json"),
        "runtime_sha256":file(&runtime.join("runtime.json"))?["sha256"],"backend":contract["backend_root"],"runtime":contract});
    validate(&args)?;
    for (name, mode) in [
        ("candidate.json", "candidate"),
        ("supervisor.json", "supervisor"),
        ("progress.json", "progress"),
        ("result.json", "result"),
    ] {
        if let Some(value) = records.get(name) {
            args["mode"] = json!(mode);
            args["value"] = value.clone();
            validate(&args)?;
        } else if archived && name == "result.json" {
            return Err("源角色归档缺少最终结果".into());
        }
    }
    if archived
        && records["result.json"]["outcome"] == "succeeded"
        && (records
            .get("progress.json")
            .is_none_or(|v| v["phase"] != "ready")
            || !records.contains_key("candidate.json")
            || !records.contains_key("supervisor.json"))
    {
        return Err("源角色成功档案缺少同代ready发布".into());
    }
    members::verify(runtime, role, id, &records, archived)?;
    for binding in bindings {
        let path = Path::new(document::text(&binding["path"], "源角色原收据路径")?);
        if common::binding(path)? != binding {
            return Err("源角色控制链在读取期间变化".into());
        }
    }
    Ok(())
}

fn file(path: &Path) -> Result<Value> {
    let value = digest::file(path)?;
    Ok(json!({"bytes":value.bytes,"sha256":value.sha256}))
}

fn validate(args: &Value) -> Result<()> {
    worker::execute(&serde_json::to_vec(&json!({"format_version":1,"kind":"ryframe-full-stack-worker","operation":"validate","arguments":args}))?).map(|_| ())
}

fn inputs(runtime: &Path, role: &str, request: &Value) -> Result<Value> {
    controller(runtime, request)?;
    let outer = read(
        &runtime
            .parent()
            .ok_or("源runtime缺少代次目录")?
            .join("request.json"),
    )?;
    crate::recovery::clone::native::seed::generation::coordinator::request(&outer)?;
    let execution = Path::new(document::text(&outer["execution_backend"], "源实际执行根")?);
    let contract = fixture_control::verify_full_stack_runtime(execution, runtime)?;
    document::exact(
        &request["environment_binding"],
        &["file", "identity"],
        "源角色冻结环境",
    )?;
    if request["environment_binding"]["file"] != outer["source_environment"] {
        return Err("源角色环境没有绑定原始source请求".into());
    }
    let environment = &outer["source_environment"];
    let path = Path::new(document::text(&environment["path"], "源角色环境路径")?);
    plain(path, false)?;
    let guard =
        crate::recovery::clone::native::target_verify::guard::Inputs::open([path.to_path_buf()])?;
    if file(path)? != json!({"bytes":environment["bytes"],"sha256":environment["sha256"]})
        || request["environment_binding"]["identity"]
            != json!(common::device_inode_for(
                path,
                &std::fs::symlink_metadata(path)?
            )?)
    {
        return Err("源角色冻结环境文件内容或完整身份变化".into());
    }
    let value = read(path)?;
    document::exact(&value, &["environment"], "源角色私有环境文件")?;
    super::super::super::environment::configured(&serde_json::from_value(
        value["environment"].clone(),
    )?)?;
    guard.check()?;
    if role == "api" {
        let url = document::text(&request["api_ready_url"], "源API就绪地址")?
            .strip_suffix("/readyz")
            .ok_or("源API就绪路径必须为/readyz")?;
        super::super::history::lineage::failure::inputs::api_address(
            execution,
            &contract["api_config_dir"],
            &json!(url),
        )?;
    }
    Ok(contract)
}

fn controller(runtime: &Path, request: &Value) -> Result<()> {
    use crate::recovery::clone::native::lifecycle::{identity, state};
    let output = runtime.parent().ok_or("源runtime缺少代次")?;
    let run = output.parent().ok_or("源代次缺少原账本")?;
    let name = output
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or("源代次名称无效")?;
    let number = name
        .strip_prefix('g')
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .ok_or("源代次缺少实际START序号")?;
    if name != format!("g{number:04}") {
        return Err("源代次不是规范START目录".into());
    }
    let manifest = common::binding(&run.join("manifest.json"))?;
    let before = common::binding(&run.join("state.json"))?;
    let saved = state::load(run, true)?;
    let mut attempt = state::attempts(&saved)?
        .iter()
        .find(|v| v["number"] == number)
        .ok_or("源角色缺少原START阶段")?
        .clone();
    if attempt["stage"] != "seed-runtime" || attempt["mode"] != "source-generation-start" {
        return Err("源角色控制器不属于原START阶段".into());
    }
    // 持久控制器绑定的是阶段开始时的完整记录，按原字段还原并核对摘要。
    attempt["status"] = json!("running");
    for key in ["finished_at", "result", "error_type"] {
        attempt[key] = Value::Null;
    }
    let (binding, owner) = state::controller_record(run, number, &attempt)?;
    identity::validate(&owner, run)?;
    if owner["manifest_sha256"] != manifest["sha256"]
        || request["controller_identity"] != owner["identity"]
    {
        return Err("源角色owner不是原START同进程锁持有者".into());
    }
    let controller = state::controller_path(run, number);
    if common::binding(&controller)? != binding
        || common::binding(&run.join("state.json"))? != before
        || common::binding(&run.join("manifest.json"))? != manifest
    {
        return Err("源角色原始控制器或账本在核验期间变化".into());
    }
    Ok(())
}

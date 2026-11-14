//! 历史 actor 只核验原授权与实际收据，不要求旧锁或启动阶段仍在运行。
use super::*;
use crate::recovery::clone::native::lifecycle::{manifest, state};
use std::collections::BTreeSet;

pub(in crate::recovery::fixture_control::services) fn load(
    backend: &Path,
    run: &Path,
    attempt: &Value,
) -> Result<Value> {
    let output = run.join("rustfs");
    let original = document::read(&output.join("actor-binding.json"))?;
    binding::verify_snapshot(&original)?;
    check(
        Path::new(document::text(
            &original["binding"]["path"],
            "原 actor 文件",
        )?) == output.join("actor.json"),
        "原 actor descriptor 不属于固定启动目录",
    )?;
    let actor: actor::Actor = serde_json::from_value(common::read_bound(
        backend,
        &original["binding"],
        "原 actor",
    )?)?;
    authorization(backend, run, attempt, &actor)?;
    receipts(&actor)?;
    let ready = members(&actor)?;
    files(&actor)?;
    let mut bindings = json!({"rustfs_actor.json":original["binding"],
        "rustfs_actor-binding.json":common::binding(&output.join("actor-binding.json"))?,
        "rustfs_members_ready":common::binding(&actor.member_path("ready"))?});
    for name in ["spawned.json", "process.json", "launch.json", "rustfs.json"] {
        if actor.path(name).try_exists()? {
            bindings[format!("rustfs_{name}")] = common::binding(&actor.path(name))?;
        }
    }
    if actor.member_path("product").try_exists()? {
        bindings["rustfs_members_product"] = common::binding(&actor.member_path("product"))?;
    }
    Ok(json!({"operation_id":actor.operation_id,"ready":ready,"bindings":bindings}))
}

fn authorization(backend: &Path, run: &Path, attempt: &Value, actor: &actor::Actor) -> Result<()> {
    check(
        std::fs::canonicalize(&actor.backend)? == std::fs::canonicalize(backend)?
            && actor.output == run.join("rustfs")
            && actor.operation_id.len() == 32
            && actor
                .operation_id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "历史 actor 的协调目录、输出或操作 ID 无效",
    )?;
    let number = attempt["number"]
        .as_u64()
        .ok_or("历史 RustFS 阶段编号无效")?;
    check(
        number == 1
            && actor.number == number
            && attempt["stage"] == "storage-target"
            && attempt["mode"] == "initial"
            && matches!(attempt["status"].as_str(), Some("failed" | "passed")),
        "历史 actor 不属于已收尾 RustFS 首代",
    )?;
    let manifest = document::read(&run.join("manifest.json"))?;
    super::super::history::validate_manifest(&manifest)?;
    let controller_path = state::controller_path(run, number);
    let controller = document::read(&controller_path)?;
    crate::recovery::clone::native::lifecycle::identity::validate(&controller["owner"], run)?;
    crate::recovery::clone::native::lifecycle::manifest::validate_binding_shape(
        &actor.owner_binding,
        "原 owner 描述符",
    )?;
    let mut running = attempt.clone();
    for (key, value) in [
        ("status", json!("running")),
        ("finished_at", Value::Null),
        ("result", Value::Null),
        ("error_type", Value::Null),
    ] {
        running[key] = value;
    }
    check(
        actor.manifest == common::binding(&run.join("manifest.json"))?
            && actor.controller == common::binding(&controller_path)?
            && controller["attempt"] == number
            && controller["attempt_sha256"] == digest::plan_hash(&running)?
            && controller["owner"]["manifest_sha256"] == actor.manifest["sha256"]
            && Path::new(document::text(&actor.tool["path"], "原 actor 工具路径")?)
                == Path::new(document::text(
                    &controller["owner"]["identity"]["executable"],
                    "原控制器程序路径",
                )?)
            && Path::new(document::text(
                &controller["owner"]["directory"],
                "原 owner 目录",
            )?) == run,
        "历史 actor 未绑定原清单、控制器和启动阶段摘要",
    )?;
    let request = common::read_bound(backend, &attempt["sources"]["request"], "原 RustFS 请求")?;
    check(
        actor.request == request
            && request["scope_id"] == manifest["scope_id"]
            && attempt["sources"]["bootstrap"] == manifest["bootstrap"]
            && Path::new(document::text(
                &actor.owner_binding["path"],
                "原 owner 绑定",
            )?) == run.join("run.lock/owner.json"),
        "历史 actor 原请求、bootstrap 或 owner 描述符变化",
    )?;
    snapshots(backend, attempt, actor, &manifest)
}

fn snapshots(
    backend: &Path,
    attempt: &Value,
    actor: &actor::Actor,
    manifest: &Value,
) -> Result<()> {
    let bootstrap = common::read_bound(backend, &manifest["bootstrap"], "原 bootstrap")?;
    let review = common::read_bound(backend, &attempt["sources"]["review"], "原审阅")?;
    let service = &review["services"]["rustfs"];
    check(
        actor.request["executable"] == review["tools"]["rustfs"]
            && actor.request["api_url"] == service["api"]
            && actor.request["console_url"] == service["console"]
            && actor.request["scope_id"] == service["scope_id"]
            && actor.request["data_directory"]["path"] == service["data_dir"]
            && digest::plan_hash(&actor.private)?
                == document::text(&bootstrap["environment_sha256"], "原私有环境摘要")?,
        "历史 actor 服务或私有环境不同于原始审阅和 bootstrap",
    )?;
    let bootstrap_path = common::bound_file(backend, &manifest["bootstrap"], "原 bootstrap")?;
    let environment = bootstrap_path
        .parent()
        .ok_or("原 bootstrap 无父目录")?
        .join("environment.json");
    let mut expected = vec![
        attempt["sources"]["request"].clone(),
        actor.manifest.clone(),
        actor.controller.clone(),
        attempt["sources"]["review"].clone(),
        actor.owner_binding.clone(),
        manifest["bootstrap"].clone(),
        common::binding(&environment)?,
    ];
    check(
        document::read(&environment)? == json!({"environment":actor.private}),
        "历史 actor 私有环境不同于原绑定",
    )?;
    for (key, name) in [
        ("access_key", "rustfs-access-key.txt"),
        ("secret_key", "rustfs-secret-key.txt"),
    ] {
        check(
            actor.request["credential_files"][key] == bootstrap["secret_files"][name],
            "历史 actor 凭据不是原 bootstrap 描述符",
        )?;
        expected.push(actor.request["credential_files"][key].clone());
    }
    let executable = Path::new(document::text(
        &actor.request["executable"]["path"],
        "原 RustFS 可执行文件",
    )?);
    check(
        digest::file(executable)?.sha256
            == document::text(&actor.request["executable"]["sha256"], "原 RustFS 摘要")?,
        "历史 RustFS 工具字节不同于原授权",
    )?;
    expected.push(common::binding(executable)?);
    check(
        actor.snapshots.len() == expected.len(),
        "历史 actor 输入集合不完整",
    )?;
    for descriptor in &expected {
        manifest::validate_binding_shape(descriptor, "历史 actor 原授权输入")?;
        let mut normalized = descriptor.clone();
        normalized["path"] = json!(manifest::path_text(
            Path::new(document::text(&descriptor["path"], "原授权输入路径")?),
            "原授权输入路径",
        )?);
        let matches = actor
            .snapshots
            .iter()
            .filter(|snapshot| snapshot["binding"] == normalized)
            .collect::<Vec<_>>();
        check(matches.len() == 1, "历史 actor 原授权输入缺失或重复")?;
        if descriptor != &actor.owner_binding {
            binding::verify_snapshot(matches[0])?;
        }
    }
    manifest::validate_binding_shape(&actor.tool, "原 actor 协调器工具")?;
    let tool = Path::new(document::text(&actor.tool["path"], "原 actor 工具路径")?);
    check(
        tool.is_absolute()
            && std::fs::symlink_metadata(tool)?.is_file()
            && !digest::linked_path(tool)?.contains(&true)
            && common::binding(tool)? == actor.tool,
        "原 actor 协调器工具路径、字节或描述符变化",
    )?;
    common::directory_identity(backend, &actor.request["data_directory"])?;
    Ok(())
}

fn members(actor: &actor::Actor) -> Result<Value> {
    let ready = document::read(&actor.member_path("ready"))?;
    document::exact(
        &ready,
        &[
            "format_version",
            "operation_id",
            "directory",
            "role",
            "scope_id",
            "supervisor",
            "monitor",
        ],
        "原 monitor ready",
    )?;
    check(
        ready["format_version"] == 1
            && ready["operation_id"] == actor.operation_id
            && ready["directory"] == json!(actor.output)
            && ready["role"] == "rustfs"
            && ready["scope_id"] == actor.request["scope_id"]
            && ready["supervisor"]["pid"] != ready["monitor"]["pid"],
        "历史 monitor ready 不属于原 actor",
    )?;
    for role in ["supervisor", "monitor"] {
        common::validate_process_identity(&ready[role])?;
        check(
            ready[role]["executable"] == actor.tool["path"],
            "历史监督成员映像不同于原协调器",
        )?;
    }
    product(actor, &ready)?;
    if actor.member_path("stopped").try_exists()? {
        let stopped = document::read(&actor.member_path("stopped"))?;
        check(
            stopped["supervisor"] == ready["supervisor"] && stopped["monitor"] == ready["monitor"],
            "历史 stopped 不是原 monitor/supervisor 代次",
        )?;
        actor::validate_members(actor, &stopped)?;
        if actor.path("rustfs.json").try_exists()? {
            let product = document::read(&actor.path("rustfs.json"))?;
            check(
                stopped["members"]
                    .as_array()
                    .is_some_and(|members| members.contains(&product["identity"])),
                "完整成员关闭证明缺少原 RustFS 产品",
            )?;
        }
    }
    for name in ["actor-cancel.json", "actor-failed.json"] {
        if !actor.path(name).try_exists()? {
            continue;
        }
        let value = document::read(&actor.path(name))?;
        let fields: &[&str] = if name == "actor-cancel.json" {
            &["operation_id"]
        } else {
            &["operation_id", "stage", "outcome", "error_type"]
        };
        document::exact(&value, fields, "原 actor 失败或取消")?;
        check(
            value["operation_id"] == actor.operation_id,
            "actor 失败或取消属于其他操作",
        )?;
        if name == "actor-failed.json" {
            check(
                value["stage"] == "supervision"
                    && matches!(value["outcome"].as_str(), Some("closed" | "unknown"))
                    && value["error_type"] == "SupervisionFailed",
                "actor 失败记录不属于当前监督合同",
            )?;
            if actor.member_path("stopped").try_exists()? {
                check(
                    value["outcome"] == "closed",
                    "actor 仍声明关闭未知，不接受成功关闭推断",
                )?;
            }
        }
    }
    Ok(ready)
}

fn product(actor: &actor::Actor, ready: &Value) -> Result<()> {
    if !actor.path("rustfs.json").try_exists()? {
        check(
            !actor.member_path("product").try_exists()?,
            "产品确认缺少原创建收据",
        )?;
        return Ok(());
    }
    let product = document::read(&actor.path("rustfs.json"))?;
    document::exact(
        &product,
        &["format_version", "role", "scope_id", "identity"],
        "原 RustFS 产品",
    )?;
    common::validate_process_identity(&product["identity"])?;
    check(
        product["format_version"] == 1
            && product["role"] == "rustfs"
            && product["scope_id"] == actor.request["scope_id"]
            && product["identity"]["executable"] == actor.request["executable"]["path"],
        "原 RustFS 产品不属于 actor",
    )?;
    if actor.member_path("product").try_exists()? {
        let ack = document::read(&actor.member_path("product"))?;
        document::exact(
            &ack,
            &["operation_id", "identity", "monitor"],
            "原产品句柄确认",
        )?;
        check(
            ack == json!({"operation_id":actor.operation_id,"identity":product["identity"],"monitor":ready["monitor"]}),
            "产品句柄确认不属于原 monitor 和操作",
        )?;
    }
    if actor.path("rustfs-tree.json").try_exists()? {
        check(
            actor.member_path("product").try_exists()?,
            "完整树缺少产品句柄确认",
        )?;
        let tree = document::read(&actor.path("rustfs-tree.json"))?;
        check(
            tree["supervisor"] == ready["supervisor"]
                && tree["monitor"] == ready["monitor"]
                && tree["operation_id"] == actor.operation_id
                && tree["process"] == product["identity"],
            "完整树不是原 actor 产品代次",
        )?;
    }
    Ok(())
}

fn files(actor: &actor::Actor) -> Result<()> {
    let mut allowed = [
        "request.json",
        "intent.json",
        "launch.json",
        "process.json",
        "ready.json",
        "spawned.json",
        "rustfs-tree.json",
        "rustfs.json",
        "stdout.log",
        "stderr.log",
        "actor.json",
        "actor-binding.json",
        "actor-cancel.json",
        "actor-failed.json",
        "actor-failure-diagnostic.json",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<BTreeSet<_>>();
    for suffix in ["ready", "product", "stopped"] {
        allowed.insert(format!(
            "rustfs-members-{}-{suffix}.json",
            actor.operation_id
        ));
    }
    for suffix in ["control", "result"] {
        allowed.insert(format!("rustfs-tree-{}-{suffix}.json", actor.operation_id));
    }
    for entry in std::fs::read_dir(&actor.output)? {
        let entry = entry?;
        check(
            entry.file_type()?.is_file()
                && allowed.contains(&entry.file_name().to_string_lossy().into_owned())
                && !digest::linked_path(&entry.path())?.contains(&true),
            "原 actor 目录包含未知文件、链接或其他操作写入",
        )?;
    }
    Ok(())
}

fn receipts(actor: &actor::Actor) -> Result<()> {
    let arguments = actor.arguments()?;
    let intent = document::read(&actor.path("intent.json"))?;
    check(
        intent
            == json!({"format_version":1,"kind":"devex-clone-storage-intent",
        "request":actor.manifest,"controller":actor.controller,"attempt":actor.number,"arguments":arguments}),
        "历史 actor intent 不属于原请求和控制阶段",
    )?;
    let product = if actor.path("rustfs.json").try_exists()? {
        document::read(&actor.path("rustfs.json"))?["identity"].clone()
    } else {
        Value::Null
    };
    if actor.path("process.json").try_exists()? {
        let process = document::read(&actor.path("process.json"))?;
        common::validate_process_identity(&process["identity"])?;
        check(
            (product.is_null() || process["identity"] == product)
                && process
                    == json!({
            "format_version":1,"role":"rustfs","scope_id":actor.request["scope_id"],"lifecycle":"running",
            "identity":process["identity"],"api_url":actor.request["api_url"],"console_url":actor.request["console_url"],
            "data_dir":actor.request["data_directory"]["path"],"intent":common::binding(&actor.path("intent.json"))?}),
            "历史 actor process 收据不属于原启动",
        )?;
        if actor.path("launch.json").try_exists()? {
            check(
                document::read(&actor.path("launch.json"))?
                    == json!({"format_version":1,
                "scope_id":actor.request["scope_id"],"identity":process["identity"],"arguments":arguments,
                "credential_files":actor.request["credential_files"]}),
                "历史 actor launch 不属于原产品",
            )?;
        }
    } else {
        check(
            !actor.path("launch.json").try_exists()?,
            "launch 缺少原 process 收据",
        )?;
    }
    if actor.path("spawned.json").try_exists()? {
        let spawned = document::read(&actor.path("spawned.json"))?;
        document::exact(&spawned, &["pid", "identity_pending"], "原产品创建意图")?;
        check(
            spawned["identity_pending"] == true
                && spawned["pid"].as_u64().is_some_and(|pid| pid > 1)
                && (product.is_null() || spawned["pid"] == product["pid"]),
            "原产品创建 PID 归属不同",
        )?;
        if actor.member_path("stopped").try_exists()? {
            let stopped = document::read(&actor.member_path("stopped"))?;
            check(
                stopped["members"].as_array().is_some_and(|members| {
                    members.iter().any(|member| member["pid"] == spawned["pid"])
                }),
                "早期创建产品未被完整成员退出证明覆盖",
            )?;
        }
    }
    Ok(())
}

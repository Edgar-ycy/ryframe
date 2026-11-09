//! 仅接受正式服务账本当前阶段中的 RustFS 请求，不接受任意命令。
use super::*;
use crate::local_test_path::validate_external_existing_file;
use crate::recovery::clone::native::{
    lifecycle::{lock, manifest, state},
    resources::storage,
};
use std::{fs, path::PathBuf};

struct Authority {
    paths: Vec<PathBuf>,
    review: Value,
    bootstrap: PathBuf,
    owner: Value,
}

fn authority(input: &Start<'_>, request_path: &Path) -> Result<Authority> {
    let manifest_path =
        manifest::bound_file(input.backend, input.manifest, "RustFS 服务清单", true)?;
    let run = manifest_path.parent().ok_or("服务清单无父目录")?;
    let service = document::read(&manifest_path)?;
    check(
        service["kind"] == "reference-fixture-service-run",
        "RustFS 必须属于正式服务账本",
    )?;
    let current = state::load(run, true)?;
    let attempt = state::running_attempt(&current)?.ok_or("RustFS 缺少当前运行阶段")?;
    if attempt["stage"] == "storage-target" {
        check(
            attempt["sources"]["request"] == common::binding(request_path)?,
            "RustFS 首代请求不是原阶段冻结输入",
        )?;
    }
    check(
        attempt["number"] == input.number
            && ((attempt["stage"] == "storage-target" && attempt["mode"] == "initial")
                || (attempt["stage"] == "fixture-services" && attempt["mode"] == "restart")),
        "RustFS 不属于允许的当前阶段",
    )?;
    let controller = manifest::bound_file(input.backend, input.controller, "RustFS 控制器", true)?;
    check(
        controller == state::controller_path(run, input.number),
        "RustFS 控制器不属于当前阶段",
    )?;
    let controlled = document::read(&controller)?;
    check(
        controlled["attempt"] == input.number
            && controlled["attempt_sha256"] == digest::plan_hash(attempt)?,
        "RustFS 控制器阶段摘要不一致",
    )?;
    let (owner_binding, owner) = lock::owner_observation(run)?.ok_or("RustFS 缺少原运行锁")?;
    check(
        controlled["owner"] == owner,
        "RustFS 控制器不属于当前持锁 owner",
    )?;
    let owner_pid = u32::try_from(owner["identity"]["pid"].as_u64().ok_or("owner PID 无效")?)?;
    check(
        common::process_identity(owner_pid)?.as_ref() == Some(&owner["identity"]),
        "RustFS 持锁控制器已退出或创建身份变化",
    )?;
    let review_binding = &attempt["sources"]["review"];
    let review_path = manifest::bound_file(input.backend, review_binding, "RustFS 审阅", true)?;
    let review = document::read(&review_path)?;
    check(
        service["scope_id"] == input.request["scope_id"]
            && review["services"]["rustfs"]["scope_id"] == input.request["scope_id"],
        "RustFS scope 与审阅不同",
    )?;
    review_request(input, &review)?;
    let bootstrap_path =
        manifest::bound_file(input.backend, &service["bootstrap"], "RustFS 环境", true)?;
    let owner_path =
        manifest::bound_file(input.backend, &owner_binding, "RustFS 运行锁 owner", true)?;
    Ok(Authority {
        paths: vec![manifest_path, controller, review_path, owner_path],
        review,
        bootstrap: bootstrap_path,
        owner: owner_binding,
    })
}

fn environment(input: &Start<'_>, authority: &Authority) -> Result<Vec<PathBuf>> {
    let review_path = &authority.paths[2];
    let review = &authority.review;
    let bootstrap_path = &authority.bootstrap;
    let names = json!([
        "mysql-client.cnf",
        "rustfs-access-key.txt",
        "rustfs-secret-key.txt",
        "redis-password.txt",
        "reset-admin-password.txt",
        "jwt-secret.txt",
        "metrics-token.txt"
    ]);
    let prepared = super::super::lifecycle::query::dispatch(
        input.backend,
        "service-prepared",
        &json!({"path":bootstrap_path,"secret_files":names}),
    )?;
    super::super::lifecycle::query::dispatch(
        input.backend,
        "service-environment",
        &json!({
        "review_file":review_path,"review":review,"environment":bootstrap_path}),
    )?;
    check(
        prepared[2] == *input.private,
        "RustFS 私有环境不同于已准备环境",
    )?;
    for (key, name) in [
        ("access_key", "rustfs-access-key.txt"),
        ("secret_key", "rustfs-secret-key.txt"),
    ] {
        check(
            input.request["credential_files"][key] == prepared[3]["secret_files"][name],
            "RustFS 请求凭据不是 bootstrap 原始绑定",
        )?;
    }
    let mut paths = vec![
        bootstrap_path.clone(),
        bootstrap_path
            .parent()
            .ok_or("环境无父目录")?
            .join("environment.json"),
    ];
    for descriptor in input.request["credential_files"]
        .as_object()
        .ok_or("RustFS 凭据清单无效")?
        .values()
    {
        paths.push(manifest::bound_file(
            input.backend,
            descriptor,
            "RustFS 凭据",
            true,
        )?);
    }
    Ok(paths)
}

pub(super) fn prepare(input: &Start<'_>) -> Result<actor::Actor> {
    common::local_directory(input.backend, input.output, "RustFS 输出")?;
    document::exact(
        input.request,
        &[
            "scope_id",
            "executable",
            "data_directory",
            "api_url",
            "console_url",
            "credential_files",
            "timeout_seconds",
        ],
        "RustFS 首代或生命周期请求",
    )?;
    check(
        input.request["timeout_seconds"] == 60,
        "正式 RustFS 就绪期限必须为 60 秒",
    )?;
    let request_path = input.output.join("request.json");
    check(
        document::read(&request_path)? == *input.request,
        "RustFS 请求不是原输出登记",
    )?;
    let authorized = authority(input, &request_path)?;
    let mut paths = vec![request_path];
    paths.extend(authorized.paths.iter().cloned());
    paths.extend(environment(input, &authorized)?);
    let executable = PathBuf::from(document::text(
        &input.request["executable"]["path"],
        "RustFS 工具",
    )?);
    validate_external_existing_file(&executable)?;
    check(
        digest::file(&executable)?.sha256 == input.request["executable"]["sha256"],
        "RustFS 工具字节不同于冻结审阅",
    )?;
    paths.push(executable);
    let snapshots = paths
        .iter()
        .map(|path| snapshot(path))
        .collect::<Result<Vec<_>>>()?;
    Ok(actor::Actor {
        backend: input.backend.into(),
        output: input.output.into(),
        request: input.request.clone(),
        private: input.private.clone(),
        manifest: input.manifest.clone(),
        controller: input.controller.clone(),
        number: input.number,
        operation_id: uuid::Uuid::new_v4().simple().to_string(),
        tool: common::binding(&std::env::current_exe()?)?,
        snapshots,
        owner_binding: authorized.owner,
    })
}

fn review_request(input: &Start<'_>, review: &Value) -> Result<()> {
    let service = &review["services"]["rustfs"];
    check(
        input.request["executable"] == review["tools"]["rustfs"]
            && input.request["api_url"] == service["api"]
            && input.request["console_url"] == service["console"],
        "RustFS 工具或端点不同于当前冻结审阅",
    )?;
    let data = common::directory_identity(input.backend, &input.request["data_directory"])?;
    check(
        data == Path::new(document::text(&service["data_dir"], "数据目录")?),
        "RustFS 数据目录不同于审阅",
    )?;
    for key in ["api_url", "console_url"] {
        common::endpoint(document::text(&input.request[key], "RustFS 地址")?)?;
    }
    check(
        input.request["api_url"] != input.request["console_url"],
        "RustFS API 与控制台必须不同",
    )?;
    document::exact(
        &input.request["credential_files"],
        &["access_key", "secret_key"],
        "RustFS 凭据",
    )?;
    let _ = storage::model::arguments(input.request)?;
    Ok(())
}

pub(in crate::recovery::fixture_control::services) fn snapshot(path: &Path) -> Result<Value> {
    check(
        !digest::linked_path(path)?.contains(&true),
        "RustFS 输入经过链接",
    )?;
    let meta = fs::symlink_metadata(path)?;
    check(
        meta.is_file() && !digest::linked(&meta),
        "RustFS 输入不是普通文件",
    )?;
    let (device, inode) = common::device_inode_for(path, &meta)?;
    Ok(json!({"binding":common::binding(path)?,"device":device,"inode":inode}))
}

pub(super) fn current_owner(actor: &actor::Actor) -> Result<()> {
    let manifest_path =
        manifest::bound_file(&actor.backend, &actor.manifest, "RustFS 服务清单", true)?;
    let run = manifest_path.parent().ok_or("服务清单无父目录")?;
    let (bound, owner) = lock::owner_observation(run)?.ok_or("RustFS 原运行锁已丢失")?;
    let controller = common::read_bound(&actor.backend, &actor.controller, "RustFS 控制器")?;
    check(
        bound == actor.owner_binding && controller["owner"] == owner,
        "RustFS 运行锁或控制器 owner 变化",
    )?;
    let pid = u32::try_from(owner["identity"]["pid"].as_u64().ok_or("owner PID 无效")?)?;
    check(
        common::process_identity(pid)?.as_ref() == Some(&owner["identity"]),
        "RustFS 持锁控制器创建身份变化",
    )
}

pub(in crate::recovery::fixture_control::services) fn verify_snapshot(value: &Value) -> Result<()> {
    let path = Path::new(document::text(&value["binding"]["path"], "冻结输入")?);
    let sampled = snapshot(path)?;
    check(sampled == *value, "RustFS 输入字节或内核文件身份变化")
}

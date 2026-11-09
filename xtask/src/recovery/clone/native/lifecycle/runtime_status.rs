//! 只读观察旧复制运行的登记产品身份；不要求后继原生整树收据。
use super::{manifest, state};
use crate::{
    Result,
    cli::{CloneRole, CloneRuntimeOptions, CloneSide},
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::{
        clone::native::{
            export::proof, json as document, resources::common, target_verify::authority,
        },
        fixture_control::{
            artifact::runtime, portable_path, read_json, services::lifecycle::readonly::process,
        },
        full_stack::worker::controller,
        inputs::domain,
        runtime::dataset_authority::reads,
    },
};
use serde_json::{Map, Value, json};
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

pub(in crate::recovery::clone::native) fn run(
    backend: &Path,
    options: &CloneRuntimeOptions,
) -> Result<Value> {
    let window = reads::Window::direct()?;
    window.finish(observe(backend, options))
}

fn observe(backend: &Path, options: &CloneRuntimeOptions) -> Result<Value> {
    if !matches!(
        options.roles.as_slice(),
        [CloneRole::Api]
            | [CloneRole::Worker]
            | [CloneRole::Api, CloneRole::Worker]
            | [CloneRole::Worker, CloneRole::Api]
    ) {
        return Err("运行状态必须选择不重复的 API 或 Worker".into());
    }
    let backend = directory(backend)?;
    let run = common::local_directory(&backend, &options.run_dir, "复制运行目录")?;
    let saved = manifest::registered(&backend, &run.join("manifest.json"))?;
    state::load(&run, false)?;
    let selected = selected(&backend, &saved.value, options.side)?;
    let directory = directory(Path::new(document::text(
        &selected["runtime_dir"],
        "已登记运行目录",
    )?))?;
    let api_url = document::text(&selected["api_url"], "已登记 API 地址")?;
    let binding = runtime::verify(&backend, &directory)?;
    proof::verify_api_address(api_url, &serde_json::to_value(&binding.table)?)?;
    let api_ready = format!("{}/readyz", api_url.trim_end_matches('/'));
    let worker_ready = document::text(&binding.receipt["worker_ready_url"], "Worker 就绪地址")?;
    let api = domain::endpoint(&json!(api_ready), "API 就绪端点", Some("/readyz"))?;
    let worker = domain::endpoint(&json!(worker_ready), "Worker 就绪端点", Some("/readyz"))?;
    if (api.host, api.port) == (worker.host, worker.port) {
        return Err("API 与 Worker 必须使用不同的明确监听端点".into());
    }
    let mut processes = Map::new();
    for role in &options.roles {
        let (url, endpoint) = match role {
            CloneRole::Api => (api_ready.as_str(), &api),
            CloneRole::Worker => (worker_ready, &worker),
        };
        processes.insert(
            role.as_str().into(),
            role_status(&directory, &binding, *role, url, endpoint)?,
        );
    }
    let current = runtime::verify(&backend, &directory)?;
    if current.receipt != binding.receipt || current.table != binding.table {
        return Err("运行登记在只读状态观察期间发生变化".into());
    }
    Ok(
        json!({"format_version":1,"kind":"devex-clone-runtime-observation","operation":"status",
        "scope_id":binding.scope,"runtime_directory":directory,"processes":processes}),
    )
}

fn selected(backend: &Path, manifest: &Value, side: CloneSide) -> Result<Value> {
    let (environment, descriptor) = match side {
        CloneSide::Source => ("source_environment", "source_request"),
        CloneSide::Target => ("target_environment", "initialized"),
    };
    let environment = common::environment(backend, &manifest[environment])?;
    let mut names = std::collections::BTreeSet::new();
    if environment
        .keys()
        .any(|name| !names.insert(name.to_uppercase()))
        || !environment.iter().any(|(name, value)| {
            (name == "APP_ENV" || cfg!(windows) && name.eq_ignore_ascii_case("APP_ENV"))
                && value == "test"
        })
    {
        return Err("运行状态要求原登记环境的 APP_ENV=test 且字段不重复".into());
    }
    let receipt = common::read_bound(backend, &manifest[descriptor], "已登记侧运行选择")?;
    Ok(match side {
        CloneSide::Source => receipt["source"].clone(),
        CloneSide::Target => receipt["generation"]["selected"].clone(),
    })
}

fn directory(path: &Path) -> Result<PathBuf> {
    validate_absolute_path(path, LocalTestPathKind::ExistingDirectory)?;
    Ok(portable_path(&path.canonicalize()?))
}

fn role_status(
    directory: &Path,
    binding: &runtime::Binding,
    role: CloneRole,
    url: &str,
    endpoint: &domain::Endpoint,
) -> Result<Value> {
    let path = directory.join(format!("{}.json", role.as_str()));
    let recorded = role_receipt(&path)?;
    if let Some(receipt) = &recorded {
        let identity = role_identity(receipt, binding, role)?;
        let pid = u32::try_from(identity["pid"].as_u64().ok_or("登记角色 PID 无效")?)?;
        if let Some(actual) = common::process_identity(pid)? {
            if actual != identity {
                return Err("登记角色 PID 已复用或创建身份变化".into());
            }
            process::listener_at(&identity, endpoint.host.parse()?, endpoint.port)?;
            let ready = controller::ready(url)?;
            return Ok(json!({"state":"running","identity":identity,"ready":ready}));
        }
    }
    authority::require_closed(url)?;
    if role_receipt(&path)? != recorded {
        return Err("角色收据在闭口观察期间变化".into());
    }
    Ok(json!({"state":"stopped","identity":null,"ready":false}))
}

fn role_receipt(path: &Path) -> Result<Option<Value>> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(Some(read_json(path, "已登记角色进程", 64 * 1024)?)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn role_identity(receipt: &Value, binding: &runtime::Binding, role: CloneRole) -> Result<Value> {
    let role = role.as_str();
    let identity = common::validate_process_identity(&receipt["identity"])?;
    let executable = document::text(&identity["executable"], "登记角色产物")?;
    let artifact = document::text(&binding.receipt["artifacts"][role]["path"], "冻结角色产物")?;
    if receipt["format_version"] != 1
        || receipt["role"] != role
        || receipt["scope_id"] != binding.scope
        || Path::new(executable) != Path::new(artifact)
    {
        return Err("登记进程收据与原角色、scope 或冻结产物不符".into());
    }
    Ok(identity)
}

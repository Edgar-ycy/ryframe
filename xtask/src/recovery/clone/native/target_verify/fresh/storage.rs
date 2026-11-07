//! fresh 登记的存储历史只允许原有受控追加，不能借收据推断当前进程存活。
use super::super::super::{
    cache, json as document,
    lifecycle::state,
    resources::{common, storage},
};
use super::super::cli;
use crate::{Result, cli::CloneSide, recovery::fixture_control::services};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(super) fn load(backend: &Path, descriptor: &Value) -> Result<PathBuf> {
    load_owned(backend, descriptor, None)
}

pub(super) fn load_owned(
    backend: &Path,
    descriptor: &Value,
    owner: Option<&super::super::guard::Generation>,
) -> Result<PathBuf> {
    document::exact(descriptor, &["path", "manifest", "state"], "fresh 存储绑定")?;
    let run = common::local_directory(
        backend,
        Path::new(document::text(&descriptor["path"], "存储目录")?),
        "fresh 存储目录",
    )?;
    let manifest = common::read_bound(backend, &descriptor["manifest"], "fresh 存储清单")?;
    if common::bound_file(backend, &descriptor["manifest"], "fresh 存储清单")?
        != run.join("manifest.json")
    {
        return Err("fresh 存储清单不属于固定目录".into());
    }
    let history = state::historical(&run, &descriptor["state"], true)?;
    let count = history["historical_attempts"]
        .as_u64()
        .and_then(|v| usize::try_from(v).ok())
        .ok_or("存储历史长度无效")?;
    if manifest["kind"] == "reference-fixture-service-run" {
        services::history::validate_manifest(&manifest)?;
        services::history::validate(&run, &history["state"], true, true)?;
        if count < 3 {
            return Err("夹具服务登记未包含完整首代".into());
        }
    } else {
        let attempts = state::attempts(&history["state"])?;
        let appended = attempts.get(count..).ok_or("存储历史前缀越界")?;
        let kinds = cli::execute(
            backend,
            &run,
            "cli-storage-append",
            &json!({"attempts":appended}),
        )?;
        if kinds["storage"] == true
            && storage::registered::registered(backend, &run, CloneSide::Target, None)?.is_none()
        {
            return Err("fresh 当前 RustFS 重启尚未完整发布".into());
        }
        if kinds["cache"] == true && cache::binding::load(backend, &run, owner)?.is_null() {
            return Err("fresh 当前 Redis 重启尚未完整发布".into());
        }
    }
    if common::binding(&run.join("state.json"))? != history["current"] {
        return Err("fresh 存储历史在核对期间变化".into());
    }
    Ok(run)
}

pub(super) fn availability(backend: &Path, run: &Path, descriptor: &Value) -> Result<Value> {
    let manifest = common::read_bound(backend, &descriptor["manifest"], "夹具存储清单")?;
    state::historical(run, &descriptor["state"], true)?;
    if manifest["kind"] != "reference-fixture-service-run" {
        return Ok(Value::Null);
    }
    services::history::validate_manifest(&manifest)?;
    let history = services::history::validate(run, &state::load(run, true)?, true, true)?;
    let reason = if history["closed"] == true {
        Some("夹具服务已正常关闭；须先执行 fixture services restart 再续作 fresh target")
    } else if !history["external_recovery"].is_null() {
        Some("夹具服务外部终止已核对但尚未重启")
    } else {
        None
    };
    Ok(
        json!({"available":reason.is_none(),"active_generation":history["active_generation"],"reason":reason}),
    )
}

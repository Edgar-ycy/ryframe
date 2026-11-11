//! 选择原 run 最新已通过的 close，并复核完整排空结果及之后的阶段边界。
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::{
    Result,
    recovery::clone::native::{
        json as document,
        lifecycle::{manifest, state},
        resources::{common, origin},
        seed::{protocol::guards, runtime_model},
        target_verify::{authority, input::request_configuration::canonical_path, published},
    },
};

const ALLOWED_AFTER_CLOSE: [(&str, &str); 21] = [
    ("seed-runtime", "source-register"),
    ("seed-runtime", "source-rebind"),
    ("seed-runtime", "source-generation-start"),
    ("seed-runtime", "source-generation-stop"),
    ("seed-runtime", "source-generation-recover"),
    ("seed-runtime", "source-export"),
    ("seed-runtime", "source-export-reconcile"),
    ("seed-runtime", "arm-input"),
    ("seed-runtime", "stop"),
    ("seed-runtime", "recover"),
    ("seed-runtime", "recover-session"),
    ("storage-target", "restart"),
    ("storage-target", "status"),
    ("storage-target", "stop"),
    ("storage-target", "recover"),
    ("cache-target", "restart"),
    ("cache-target", "status"),
    ("cache-target", "stop"),
    ("cache-target", "recover"),
    ("cache-target", "reconcile"),
    ("cache-target", "resume"),
];

pub(crate) fn latest(
    backend: &Path,
    run: &Path,
    state: &Value,
    runtime: &Path,
    handoff: &Value,
    current: u64,
) -> Result<Value> {
    common::local_directory(backend, run, "seed source-register 运行目录")?;
    let attempts = state::attempts(state)?;
    let record = attempts
        .iter()
        .rfind(|item| {
            item["stage"] == "seed-runtime"
                && item["mode"] == "close"
                && item["number"].as_u64() != Some(current)
        })
        .ok_or("最新 seed close 尚未成功发布")?;
    if record["status"] != "passed" {
        return Err("最新 seed close 尚未成功发布，不能回退历史成功".into());
    }
    let number = record["number"]
        .as_u64()
        .filter(|number| *number > 0)
        .ok_or("seed close 阶段序号无效")?;
    let descriptor = record.get("result").ok_or("seed close 缺少外层结果绑定")?;
    let path = common::bound_file(backend, descriptor, "seed close 外层结果")?;
    if path != run.join("results").join(format!("{number:04}.json")) {
        return Err("seed close 外层结果不属于固定 run attempt".into());
    }
    let value = document::read(&path)?;
    guards::bound_result(
        backend,
        &json!({"value":value,"runtime":runtime,"handoff":handoff,"in_progress":false}),
    )?;
    if value["status"] != "seed_runtime_closed" || value["outbox_drained"] != true {
        return Err("seed close 未取得完整 outbox 排空终态".into());
    }
    for attempt in attempts {
        let later = attempt["number"].as_u64().ok_or("seed 历史阶段序号无效")?;
        if number < later && later < current {
            let pair = (
                document::text(&attempt["stage"], "seed 后续阶段")?,
                document::text(&attempt["mode"], "seed 后续模式")?,
            );
            if !ALLOWED_AFTER_CLOSE.contains(&pair) {
                return Err("seed close 后出现未重新排空的阶段，不能登记复制源".into());
            }
        }
    }
    if common::bound_file(backend, descriptor, "seed close 外层结果")? != path
        || common::binding(&path)? != *descriptor
    {
        return Err("seed close 结果在读取期间变化；保留现场，禁止自动重放".into());
    }
    Ok(descriptor.clone())
}

/// 调用方持有复制运行的进程守卫，只读核验交接前的关闭结果及原服务代次。
pub(crate) fn handoff(
    backend: &Path,
    close_result: &Value,
    storage_run: &Path,
    services: &Value,
) -> Result<(PathBuf, Value)> {
    let result = common::bound_file(backend, close_result, "RustFS 交接的 seed close 结果")?;
    let run = result
        .parent()
        .filter(|path| path.ends_with("results"))
        .and_then(Path::parent)
        .ok_or("RustFS 交接必须选择复制运行的 results 收据")?;
    let run = canonical_path(&common::local_directory(
        backend,
        run,
        "RustFS 交接复制运行",
    )?)?;
    let manifest_path = run.join("manifest.json");
    let manifest_binding = common::binding(&manifest_path)?;
    let manifest = manifest::load(backend, &manifest_path)?.value;
    if manifest["copy_stage"] != "source_to_seed" {
        return Err("RustFS 交接只接受原 source_to_seed 复制运行".into());
    }
    let state_path = run.join("state.json");
    let state_binding = common::binding(&state_path)?;
    let saved = state::load(&run, true)?;
    if state::running_attempt(&saved)?.is_some() {
        return Err("RustFS 交接时复制运行仍有未结束阶段".into());
    }
    let last = state::attempts(&saved)?
        .last()
        .ok_or("RustFS 交接缺少 seed close 阶段")?;
    if last["result"] != *close_result || last["status"] != "passed" {
        return Err("RustFS 交接只能接续复制账本末尾的成功 seed close".into());
    }
    let next = last["number"]
        .as_u64()
        .and_then(|number| number.checked_add(1))
        .ok_or("RustFS 交接缺少有效复制阶段序号")?;
    let runtime = runtime_model::inputs(backend, &run)?;
    if latest(
        backend,
        &run,
        &saved,
        &runtime.runtime,
        &runtime.handoff,
        next,
    )? != *close_result
    {
        return Err("RustFS 交接必须绑定当前最新的 seed close 结果".into());
    }
    let close = common::read_bound(backend, close_result, "RustFS 交接的 seed close 结果")?;
    let initialized =
        common::bound_file(backend, &manifest["initialized"], "RustFS 交接的目标初始化")?;
    let markers =
        published::observe_unlocked_history(backend, &initialized, |initial, request| {
            fixture_origin(
                backend,
                &initialized,
                &manifest,
                storage_run,
                services,
                &initial,
                &request,
            )?;
            let selected = &initial["generation"]["selected"];
            require_quiet(backend, selected, &runtime, &close)?;
            marker_contract(selected, &request)
        })?;
    if common::binding(&state_path)? != state_binding
        || common::binding(&manifest_path)? != manifest_binding
        || common::binding(&result)? != *close_result
    {
        return Err("RustFS 交接期间复制账本或收据变化".into());
    }
    common::bound_file(backend, &close["handoff"], "seed close 固定交接")?;
    common::bound_file(backend, &manifest["initialized"], "RustFS 交接的目标初始化")?;
    Ok((run, markers))
}

fn fixture_origin(
    backend: &Path,
    initialized: &Path,
    manifest: &Value,
    storage_run: &Path,
    services: &Value,
    initial: &Value,
    request: &Value,
) -> Result<()> {
    let target = initialized.parent().ok_or("RustFS 交接缺少初始化目标")?;
    let workspace = target.parent().ok_or("RustFS 交接缺少 fresh 登记目录")?;
    let registration_path = common::common_file(
        backend,
        &workspace.join("registration.json"),
        "RustFS 交接的 fresh 登记",
    )?;
    let registration_binding = common::binding(&registration_path)?;
    let registration =
        common::read_bound(backend, &registration_binding, "RustFS 交接的 fresh 登记")?;
    let storage = &registration["storage_run"];
    document::exact(
        storage,
        &["path", "manifest", "state"],
        "RustFS 交接的存储绑定",
    )?;
    let registered = common::local_directory(
        backend,
        Path::new(document::text(&storage["path"], "RustFS 交接存储目录")?),
        "RustFS 交接存储目录",
    )?;
    let service_run = common::local_directory(backend, storage_run, "原夹具服务运行")?;
    if canonical_path(&registered)? != canonical_path(&service_run)?
        || registration["format_version"] != 1
        || registration["kind"] != "devex-clone-fresh-target-registration"
        || registration["target_directory"] != json!(target)
        || registration["environment"] != manifest["target_environment"]
        || request["side"] != "seed"
    {
        return Err("RustFS 交接不属于同一原 fresh 目标和夹具服务运行".into());
    }
    let fixture_manifest = registered.join("manifest.json");
    let fixture = common::read_bound(backend, &storage["manifest"], "原夹具服务清单")?;
    let mut review = document::object(&request["review"], "RustFS 交接原审阅")?.clone();
    review.remove("canonical_sha256");
    if common::binding(&fixture_manifest)? != storage["manifest"]
        || fixture["kind"] != "reference-fixture-service-run"
        || fixture["review"] != Value::Object(review)
    {
        return Err("RustFS 交接的原夹具清单或审阅不一致".into());
    }
    let history = state::historical(&registered, &storage["state"], true)?;
    let stages = [
        ("storage-target", "initial"),
        ("cache-target", "initial"),
        ("fixture-buckets", "prepare"),
    ];
    let first = state::attempts(&history["state"])?
        .get(..stages.len())
        .ok_or("RustFS 交接缺少首代夹具阶段")?;
    if history["historical_attempts"]
        .as_u64()
        .is_none_or(|count| count < 3)
        || first.iter().zip(stages).any(|(attempt, (stage, mode))| {
            attempt["stage"] != stage || attempt["mode"] != mode || attempt["status"] != "passed"
        })
        || common::binding(&registered.join("state.json"))? != history["current"]
        || common::binding(&registration_path)? != registration_binding
    {
        return Err("RustFS 交接缺少固定首代夹具阶段或登记发生变化".into());
    }
    let original = &request["storage"];
    let rustfs = &original["rustfs"];
    let redis = &original["redis"];
    if !rustfs.is_object()
        || !redis.is_object()
        || initial["generation"]["storage"] != *original
        || services["origin"]["storage"] != *rustfs
        || services["origin"]["redis"] != *redis
        || services["runtime"]["redis"] != *redis
        || services["tree"]["process"] != rustfs["identity"]
        || ["identity", "sha256", "process_receipt", "launch_receipt"]
            .iter()
            .any(|field| services["storage"][*field] != rustfs[*field])
    {
        return Err("RustFS 交接要求原初始化和当前夹具使用同一 RustFS 及 Redis 代次".into());
    }
    Ok(())
}

fn require_quiet(
    backend: &Path,
    selected: &Value,
    seed: &runtime_model::Inputs,
    close: &Value,
) -> Result<()> {
    let original = origin::runtime_directory(backend, selected)?;
    let scope = document::text(&selected["scope_id"], "RustFS 交接业务 scope")?;
    let original_api = &seed.handoff["original_api"];
    if seed.handoff["scope_id"] != scope
        || seed.handoff["api_url"] != selected["api_url"]
        || common::bound_file(backend, original_api, "原 post-copy API 收据")?
            != original.join("api.json")
    {
        return Err("RustFS 交接的业务运行范围或原 API 收据不一致".into());
    }
    require_stopped_role(
        &common::read_bound(backend, original_api, "原 post-copy API 收据")?,
        "api",
        scope,
    )?;
    if let Some(worker) = origin::receipt_file(&original.join("worker.json"))? {
        require_stopped_role(&worker, "worker", scope)?;
    }
    for role in ["api", "worker"] {
        let filename = format!("{role}.json");
        let receipt = origin::receipt_file(&seed.runtime.join(&filename))?
            .ok_or("RustFS 交接缺少 seed 角色进程收据")?;
        if receipt["identity"] != close["process_identities"][role] {
            return Err("RustFS 交接的 seed 进程不同于已关闭代次".into());
        }
        require_stopped_role(&receipt, role, scope)?;
    }
    if runtime_model::stopped(&seed.runtime, &seed.handoff)? != close["stopped_evidence"] {
        return Err("RustFS 交接的生产者停止证据不同于原 close".into());
    }
    for field in ["api_url", "worker_ready_url"] {
        authority::require_closed(document::text(&selected[field], "RustFS 交接业务端口")?)?;
    }
    common::bound_file(backend, original_api, "原 post-copy API 收据后像")?;
    Ok(())
}

fn require_stopped_role(receipt: &Value, role: &str, scope: &str) -> Result<()> {
    if receipt["role"] != role || receipt["scope_id"] != scope {
        return Err("RustFS 交接的业务进程角色或 scope 不一致".into());
    }
    common::require_recorded_stopped(&receipt["identity"])
}

fn marker_contract(selected: &Value, request: &Value) -> Result<Value> {
    let scope = document::text(&request["target"]["scope_id"], "Redis 标记 scope")?;
    let id = document::text(&request["id"], "Redis 标记初始化 ID")?;
    let markers = json!({
        "ownership_key": format!("ryframe:{{{scope}}}:.ryframe-owner"),
        "ownership_value": format!("ryframe-owner:v1:{scope}:redis"),
        "sentinel_key": format!("ryframe:devex-fresh:{scope}:sentinel"),
        "sentinel_value": format!("devex-fresh:{id}"),
    });
    if selected["scope_id"] != scope
        || ["ownership_key", "ownership_value"]
            .iter()
            .any(|field| selected["redis"][*field] != markers[*field])
        || ["sentinel_key", "sentinel_value"]
            .iter()
            .any(|field| request["reset"][*field] != markers[*field])
    {
        return Err("RustFS 交接的 Redis 标记不同于原初始化合同".into());
    }
    Ok(markers)
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/clone/native/seed/source_registration/close.rs"]
mod tests;

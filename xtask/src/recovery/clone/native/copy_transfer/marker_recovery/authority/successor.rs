use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{Target, common, document, state};
use crate::{
    Result,
    recovery::{clone::native::digest, fixture_control::services::history},
};

mod continued;
#[path = "successor/incident.rs"]
mod incident;
#[path = "successor/redis_incident.rs"]
mod redis_incident;

#[derive(PartialEq)]
pub(super) struct Fixture {
    pub(super) value: Value,
    pub(super) files: Vec<PathBuf>,
}

struct Evidence<'a> {
    backend: &'a Path,
    files: Vec<PathBuf>,
}

impl Evidence<'_> {
    fn read(&mut self, descriptor: &Value, label: &str) -> Result<Value> {
        self.files
            .push(common::bound_file(self.backend, descriptor, label)?);
        common::read_bound(self.backend, descriptor, label)
    }
}

struct Restart<'a> {
    result: &'a Value,
    outer: Value,
    generation: Value,
    receipt: Value,
    request: Value,
}

impl<'a> Restart<'a> {
    fn read(evidence: &mut Evidence<'_>, row: &'a Value) -> Result<Self> {
        let result = &row["result"];
        let outer = evidence.read(result, "已登记夹具重启")?;
        evidence.read(&outer["controller"], "原重启控制器")?;
        let receipt = outer["generation"].clone();
        let generation = evidence.read(&receipt, "已登记夹具后继代次")?;
        let request = evidence.read(&generation["redis"]["request"], "原 Redis 重启请求")?;
        Ok(Self {
            result,
            outer,
            generation,
            receipt,
            request,
        })
    }
}

pub(super) fn load(
    backend: &Path,
    run: &Path,
    target: &Target,
    completed: bool,
) -> Result<Fixture> {
    let manifest = common::binding(&run.join("manifest.json"))?;
    history::validate_manifest(&common::read_bound(backend, &manifest, "原夹具服务清单")?)?;
    let saved = state::load(run, true)?;
    let ledger = history::validate(run, &saved, true, true)?;
    let all_attempts = state::attempts(&saved)?;
    let (ledger, attempts, fifth) = continued::split(&ledger, all_attempts, completed)?;
    if completed && fifth.is_none() {
        return Err("完整复制后的标记恢复只接受原第四代的第五代后继".into());
    }
    let attempts = attempts.as_slice();
    let (first, second, third, redis) = restarts(&ledger, attempts)?;
    let mut evidence = Evidence {
        backend,
        files: Vec::new(),
    };
    let first = Restart::read(&mut evidence, first)?;
    let initial = &target.initialized["generation"]["storage"]["redis"];
    let external = first_link(&mut evidence, initial, &first)?;
    let Some(second) = second else {
        return fixture(evidence, target, manifest, &first, external, None);
    };
    let second = Restart::read(&mut evidence, second)?;
    let lineage = second_link(&mut evidence, run, attempts, initial, &first, &second)?;
    let Some(third) = third else {
        return fixture(evidence, target, manifest, &second, external, Some(lineage));
    };
    let third = Restart::read(&mut evidence, third)?;
    let incident = incident::link(
        &mut evidence,
        attempts,
        initial,
        &second,
        &third,
        &lineage["second_runtime"],
    )?;
    let mut prior = fixture_binding(&manifest, &third, &external, Some(lineage));
    prior["lineage"]["reboot_incident"] = incident;
    let Some(redis_row) = redis else {
        return finish_fixture(evidence, target, &third, prior);
    };
    let redis = Restart::read(&mut evidence, redis_row)?;
    let mut accepted =
        redis_incident::link(&mut evidence, run, initial, &third, redis_row, &redis)?;
    accepted["previous_fixture_sha256"] = json!(digest::plan_hash(&prior)?);
    let mut lineage = prior["lineage"].clone();
    lineage["accepted_redis_failure"] = accepted;
    let current = fixture_binding(&manifest, &redis, &external, Some(lineage));
    let Some(fifth) = &fifth else {
        return finish_fixture(evidence, target, &redis, current);
    };
    let active = Restart::read(&mut evidence, fifth)?;
    let mut accepted = continued::link(&mut evidence, run, initial, &redis, fifth, &active)?;
    accepted["previous_fixture_sha256"] = json!(digest::plan_hash(&current)?);
    let mut lineage = current["lineage"].clone();
    lineage["post_copy_redis_failure"] = accepted;
    let current = fixture_binding(&manifest, &active, &external, Some(lineage));
    finish_fixture(evidence, target, &active, current)
}

fn fixture(
    evidence: Evidence<'_>,
    target: &Target,
    manifest: Value,
    active: &Restart<'_>,
    external: Value,
    lineage: Option<Value>,
) -> Result<Fixture> {
    let value = fixture_binding(&manifest, active, &external, lineage);
    finish_fixture(evidence, target, active, value)
}

fn finish_fixture(
    mut evidence: Evidence<'_>,
    target: &Target,
    active: &Restart<'_>,
    value: Value,
) -> Result<Fixture> {
    let selected = &target.storage.value["redis"];
    if *selected != value["redis"]
        || target.cache.as_ref() != Some(&json!({"redis":selected,"generation":active.result}))
    {
        return Err("目标 Redis 选择不属于同一原夹具后继".into());
    }
    evidence.files.sort_unstable();
    evidence.files.dedup();
    Ok(Fixture {
        value,
        files: evidence.files,
    })
}

fn fixture_binding(
    manifest: &Value,
    active: &Restart<'_>,
    external: &Value,
    lineage: Option<Value>,
) -> Value {
    let selected = &active.generation["redis"]["runtime"]["redis"];
    let mut value = json!({"manifest":manifest,"restart":active.result,"generation":active.receipt,
        "termination":external["result"],"termination_evidence":external["proof"],"redis":selected});
    if let Some(mut lineage) = lineage {
        lineage["initial_external_runtime"] = external["runtime"].clone();
        value["lineage"] = lineage;
    }
    value
}

pub(super) type RestartRows<'a> = (
    &'a Value,
    Option<&'a Value>,
    Option<&'a Value>,
    Option<&'a Value>,
);

pub(super) fn restarts<'a>(ledger: &Value, attempts: &'a [Value]) -> Result<RestartRows<'a>> {
    let mut restarts = attempts.iter().filter(|row| {
        row["stage"] == "fixture-services" && row["mode"] == "restart" && row["status"] == "passed"
    });
    let first = restarts.next().ok_or("原夹具尚未发布成功重启")?;
    let second = restarts.next();
    let third = restarts.next();
    let redis = redis_incident::select(attempts, third)?;
    if restarts.next().is_some()
        || ledger["active_generation"] != redis.or(third).or(second).unwrap_or(first)["result"]
        || third.is_some_and(|row| {
            row["result"] == first["result"] || row["result"] == second.unwrap_or(first)["result"]
        })
        || ledger["closed"] != false
        || !ledger["external_recovery"].is_null()
        || !ledger["rustfs_handoff"].is_null()
    {
        return Err(if redis.is_some() {
            "Redis 标记恢复要求登记的 Redis 单独后继是唯一当前活跃代次"
        } else {
            "Redis 标记恢复只接受原夹具当前活跃且唯一的前三次重启代次"
        }
        .into());
    }
    Ok((first, second, third, redis))
}

fn first_link(evidence: &mut Evidence<'_>, initial: &Value, first: &Restart<'_>) -> Result<Value> {
    let result = &first.generation["predecessor"];
    let predecessor = evidence.read(result, "已登记外部终止核对")?;
    evidence.read(&predecessor["controller"], "原外部终止控制器")?;
    let proof = evidence.read(&predecessor["evidence"], "原 Windows 终止观察")?;
    let observed = &proof["observation"]["evidence"];
    evidence.read(&observed["rustfs_tree"], "原外部终止 RustFS 进程树")?;
    evidence.read(
        &observed["rustfs_monitor_ready"],
        "原外部终止 RustFS 监督器",
    )?;
    let runtime = &observed["redis_runtime"];
    let previous = evidence.read(runtime, "原 Windows Redis 代次")?;
    let selected = &first.generation["redis"]["runtime"]["redis"];
    check_first(
        initial,
        selected,
        &previous,
        &first.request,
        &first.generation,
        &proof,
    )?;
    Ok(json!({"result":result,"proof":predecessor["evidence"],"runtime":runtime}))
}

pub(super) fn check_first(
    initial: &Value,
    selected: &Value,
    previous: &Value,
    request: &Value,
    generation: &Value,
    proof: &Value,
) -> Result<()> {
    if previous["redis"] != *initial
        || generation["previous_generation"]["kind"] != "initial"
        || generation["redis"]["runtime"]["redis"] != *selected
        || proof["format_version"] != 2
        || proof["kind"] != "reference-fixture-external-termination"
        || proof["observation"]["evidence"]["redis_external"]["kind"]
            != "windows-redis-external-termination"
    {
        return Err("原 Windows Redis 外部终止或同端点后继不完整".into());
    }
    check_redis_successor(initial, selected, previous, request)
}

fn second_link(
    evidence: &mut Evidence<'_>,
    run: &Path,
    attempts: &[Value],
    initial: &Value,
    first: &Restart<'_>,
    second: &Restart<'_>,
) -> Result<Value> {
    let result = &second.generation["predecessor"];
    let row = close_row(attempts, result)?;
    let outer = evidence.read(result, "第二次重启的原失败关闭和解")?;
    evidence.read(&outer["controller"], "原失败关闭和解控制器")?;
    let reconciliation = evidence.read(&outer["reconciliation"], "原失败关闭和解证据")?;
    let recovery = evidence.read(
        &reconciliation["evidence"]["close_recovery"],
        "原非零退出证明",
    )?;
    let candidate = &recovery["candidate"];
    let previous = evidence.read(&candidate["runtime"], "第一次重启的原 Redis 运行收据")?;
    check_close(
        &outer,
        &reconciliation,
        &recovery,
        &previous,
        (first.result, &first.generation),
        &second.generation,
    )?;
    check_first_runtime_path(
        run,
        first.generation["attempt"]
            .as_u64()
            .ok_or("第一次重启缺少阶段序号")?,
        &previous,
        &candidate["runtime"],
    )?;
    if evidence.read(&candidate["request"], "第一次重启的原 Redis 请求")? != first.request
        || row["number"] != reconciliation["attempt"]
    {
        return Err("原失败关闭未绑定第一次重启的登记目录、请求和运行收据".into());
    }
    let selected = &second.generation["redis"]["runtime"]["redis"];
    check_redis_successor(initial, selected, &previous, &second.request)?;
    protect_close_files(evidence, candidate, &recovery)?;
    let second_runtime = &second.generation["redis"]["runtime"];
    let output = Path::new(document::text(
        &second_runtime["output"],
        "当前 Redis 运行目录",
    )?);
    let second_runtime_binding = common::binding(&output.join("runtime.json"))?;
    if evidence.read(&second_runtime_binding, "当前 Redis 运行收据")? != *second_runtime {
        return Err("当前 Redis 运行收据与第二次重启不同".into());
    }
    Ok(
        json!({"first_restart":first.result,"first_generation":first.receipt,
        "first_request":first.generation["redis"]["request"],"first_runtime":candidate["runtime"],
        "first_exit":candidate["exit"],"close_reconciliation":result,
        "close_reconciliation_evidence":outer["reconciliation"],"close_recovery":reconciliation["evidence"]["close_recovery"],
        "second_request":second.generation["redis"]["request"],"second_runtime":second_runtime_binding}),
    )
}

pub(super) fn check_first_runtime_path(
    run: &Path,
    attempt: u64,
    previous: &Value,
    binding: &Value,
) -> Result<()> {
    let output = Path::new(document::text(
        &previous["output"],
        "第一次重启的 Redis 目录",
    )?);
    let expected = run.join(format!("lifecycle-{attempt:04}")).join("redis");
    let path = Path::new(document::text(
        &binding["path"],
        "第一次重启的 Redis 运行收据路径",
    )?);
    if output != expected || path != output.join("runtime.json") {
        return Err("原失败关闭未绑定第一次重启的登记目录和运行收据".into());
    }
    Ok(())
}

pub(super) fn close_row<'a>(attempts: &'a [Value], result: &Value) -> Result<&'a Value> {
    let mut matching = attempts.iter().filter(|row| {
        row["stage"] == "fixture-services"
            && row["mode"] == "reconcile"
            && row["status"] == "passed"
            && row["result"] == *result
    });
    match (matching.next(), matching.next()) {
        (Some(row), None) => Ok(row),
        _ => Err("第二次重启必须绑定唯一已通过的失败关闭和解".into()),
    }
}

pub(super) fn check_close(
    outer: &Value,
    reconciliation: &Value,
    recovery: &Value,
    previous: &Value,
    first: (&Value, &Value),
    second: &Value,
) -> Result<()> {
    let candidate = &recovery["candidate"];
    let (first_result, first_generation) = first;
    if second["previous_generation"] != *first_result
        || outer["status"] != "failed_close_reconciled"
        || reconciliation["kind"] != "reference-fixture-failed-close-reconciliation"
        || recovery["format_version"] != 2
        || recovery["kind"] != "reference-fixture-close-recovery"
        || candidate["failed_attempt"] != outer["failed_attempt"]
        || !same_request(&candidate["request"], &first_generation["redis"]["request"])?
        || *previous != first_generation["redis"]["runtime"]
        || candidate["prior_windows_redis_exit"]["returncode"]
            .as_i64()
            .and_then(|code| i32::try_from(code).ok())
            .is_none_or(|code| code == 0)
    {
        return Err("第二次重启缺少第一次重启同代的 Windows Redis 非零退出和解".into());
    }
    Ok(())
}

pub(super) fn same_request(left: &Value, right: &Value) -> Result<bool> {
    for descriptor in [left, right] {
        document::exact(descriptor, &["path", "bytes", "sha256"], "Redis 请求绑定")?;
    }
    let left_path = Path::new(document::text(&left["path"], "原 Redis 请求路径")?);
    let right_path = Path::new(document::text(&right["path"], "重启 Redis 请求路径")?);
    Ok(left_path == right_path
        && left["bytes"] == right["bytes"]
        && left["sha256"] == right["sha256"])
}

fn protect_close_files(
    evidence: &mut Evidence<'_>,
    candidate: &Value,
    recovery: &Value,
) -> Result<()> {
    for descriptor in document::object(&candidate["redis_files"], "原 Redis 退出文件集合")?.values()
    {
        evidence.read(descriptor, "原 Redis 退出文件")?;
    }
    for descriptor in [
        &candidate["exit"],
        &candidate["failed_controller"],
        &candidate["rustfs_tree"],
        &candidate["prior_windows_redis_exit"]["rustfs_members"],
        &recovery["rustfs_members"],
    ] {
        evidence.read(descriptor, "原关闭和解绑定文件")?;
    }
    Ok(())
}

pub(super) fn check_redis_successor(
    initial: &Value,
    selected: &Value,
    previous: &Value,
    request: &Value,
) -> Result<()> {
    let mut allowed = initial.clone();
    for name in ["pid", "started", "run_id"] {
        allowed[name] = selected[name].clone();
    }
    if initial["platform"] != "windows"
        || selected["platform"] != "windows"
        || *selected != allowed
        || initial["run_id"] == selected["run_id"]
        || initial["started"] == selected["started"]
        || previous["redis"]["run_id"] == selected["run_id"]
        || previous["redis"]["started"] == selected["started"]
        || previous["format_version"] != 3
        || request["format_version"] != 3
        || request["previous_identity"] != previous["process"]
        || request["previous_run_id"] != previous["redis"]["run_id"]
    {
        return Err("Windows Redis 后继与原端点、配置或精确前代身份不同".into());
    }
    Ok(())
}

use super::*;
use crate::recovery::{
    clone::native::{
        cache::{redis_reboot_evidence, redis_reboot_incident},
        resources::common,
    },
    source::evidence::{host_boot_proof, verify_host_boot_proof},
};

const BOOT_DRIFT_TICKS: u64 = 60 * 10_000_000;

pub(super) fn observe(
    runtime: &mut dyn Runtime,
    context: &Context,
    services: &Value,
    previous: Option<&Value>,
) -> Result<Option<Value>> {
    let tree = &services["tree"];
    let cache = &services["runtime"];
    if tree["format_version"] != 2
        || cache["format_version"] != 3
        || !observation::receipt(tree, "stopped")?.try_exists()?
        || !Path::new(text(cache, "output")?)
            .join("failed.json")
            .try_exists()?
    {
        return Ok(None);
    }

    // 不在普通关闭或普通外部终止失败后宽泛降级；仅匹配两份事故收据。
    if read(&observation::receipt(tree, "stopped")?)?["error_type"] != "MemberCleanupFailed"
        || read(&Path::new(text(cache, "output")?).join("failed.json"))?["cleanup_completed"]
            != false
    {
        return Ok(None);
    }
    let now = host_boot_proof()?;
    let recorded = previous
        .and_then(|value| value["termination"]["evidence"].get("host_reboot"))
        .unwrap_or(&now);
    let boot = verify_host_boot_proof(recorded)?;
    check_same_boot(boot, &now)?;
    let evidence = live_evidence(runtime, context, services, boot, recorded)?;
    check(
        evidence == live_evidence(runtime, context, services, boot, recorded)?,
        "主机重启核对期间任一服务事故证据变化",
    )?;
    check_same_boot(boot, &host_boot_proof()?)?;
    Ok(Some(
        json!({"redis":"stopped","rustfs":"external-termination-unreconciled",
        "termination":termination(evidence)}),
    ))
}

fn check_same_boot(boot: u64, current: &Value) -> Result<()> {
    check(
        boot.abs_diff(verify_host_boot_proof(current)?) <= BOOT_DRIFT_TICKS,
        "当前主机启动刻度与事故核对原件不一致",
    )
}

fn live_evidence(
    runtime: &mut dyn Runtime,
    context: &Context,
    services: &Value,
    boot: u64,
    proof: &Value,
) -> Result<Value> {
    let rustfs = observation::rustfs_reboot_incident(runtime, &context.backend, services, boot)?;
    let redis = redis_reboot_incident(
        &context.backend,
        &services["requests"]["redis"],
        &services["runtime"],
        boot,
    )?;
    Ok(evidence(proof, rustfs, redis))
}

fn evidence(proof: &Value, rustfs: Value, redis: Value) -> Value {
    json!({"host_reboot":proof,"rustfs_incident":rustfs,"redis_incident":redis})
}

fn termination(evidence: Value) -> Value {
    json!({"status":"external-termination-unreconciled",
        "identities":{"supervisor":"missing","monitor":"missing","process":"missing"},
        "normal_shutdown_proof":null,"evidence":evidence})
}

pub(super) fn verify_historical(
    run: &Path,
    rustfs: &Path,
    redis: &Path,
    observation: &Value,
) -> Result<()> {
    // 执行源码可能是嵌套产品树；历史证据属于当前命令的原协调仓库。
    let backend = crate::workspace::root_dir();
    common::local_directory(&backend, run, "原服务事故账本")?;
    let proof = &observation["evidence"]["host_reboot"];
    let boot = verify_host_boot_proof(proof)?;
    let tree_path = rustfs.join("rustfs-tree.json");
    let services = json!({"tree":read(&tree_path)?,
        "requests":{"rustfs":read(&rustfs.join("request.json"))?},
        "evidence":{"rustfs_tree":binding(&tree_path)?}});
    let rustfs = observation::rustfs_reboot_evidence(&backend, &services, boot)?;
    let redis = redis_reboot_evidence(
        &backend,
        &read(&redis.join("request.json"))?,
        &read(&redis.join("runtime.json"))?,
        boot,
    )?;
    check(
        *observation == termination(evidence(proof, rustfs, redis)),
        "历史主机重启核对与原服务事故证据不同",
    )
}

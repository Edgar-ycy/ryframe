//! 第三次重启只消费服务账本已经核验的 Windows 主机重启事故。
use std::path::Path;

use serde_json::{Value, json};

use super::{Evidence, Restart, check_redis_successor, common, document};
use crate::Result;

const RUSTFS_FILES: &[&str] = &[
    "tree",
    "ready",
    "product",
    "member_failure",
    "actor_failure",
    "diagnostic",
];

pub(super) fn link(
    evidence: &mut Evidence<'_>,
    attempts: &[Value],
    initial: &Value,
    second: &Restart<'_>,
    third: &Restart<'_>,
    second_runtime: &Value,
) -> Result<Value> {
    let result = &third.generation["predecessor"];
    let row = recover_row(attempts, result)?;
    let outer = evidence.read(result, "第三次重启的事故恢复结果")?;
    evidence.read(&outer["controller"], "原事故恢复控制器")?;
    let proof = evidence.read(&outer["evidence"], "原 Windows 重启事故收据")?;
    let incident = check_proof(&outer, &proof, second_runtime)?;
    let previous = evidence.read(
        &incident["redis_incident"]["runtime"],
        "第二代 Redis 原运行收据",
    )?;
    check_link(initial, &previous, second, third, row, incident)?;

    let rustfs = &incident["rustfs_incident"];
    for name in RUSTFS_FILES {
        evidence.read(&rustfs[*name], "原 RustFS 重启事故文件")?;
    }
    evidence.read(
        &incident["redis_incident"]["failure"],
        "原 Redis 事故失败收据",
    )?;
    let runtime = protect_third(evidence, third)?;
    Ok(json!({
        "second_restart": second.result,
        "second_generation": second.receipt,
        "second_runtime": second_runtime,
        "external_recovery": result,
        "external_termination": outer["evidence"],
        "host_reboot": incident["host_reboot"],
        "rustfs_incident": rustfs,
        "redis_incident": incident["redis_incident"],
        "third_request": third.generation["redis"]["request"],
        "third_runtime": runtime,
    }))
}

fn recover_row<'a>(attempts: &'a [Value], result: &Value) -> Result<&'a Value> {
    let mut matching = attempts.iter().filter(|row| {
        row["stage"] == "fixture-services"
            && row["mode"] == "recover"
            && row["status"] == "passed"
            && row["result"] == *result
    });
    match (matching.next(), matching.next()) {
        (Some(row), None) => Ok(row),
        _ => Err("第三次重启必须绑定同一账本唯一通过的事故恢复".into()),
    }
}

fn check_proof<'a>(outer: &Value, proof: &'a Value, second_runtime: &Value) -> Result<&'a Value> {
    let observation = &proof["observation"];
    let incident = &observation["evidence"];
    document::exact(
        incident,
        &["host_reboot", "rustfs_incident", "redis_incident"],
        "原 Windows 重启事故证据",
    )?;
    let rustfs = &incident["rustfs_incident"];
    document::exact(
        rustfs,
        &[
            "tree",
            "ready",
            "product",
            "member_failure",
            "actor_failure",
            "diagnostic",
            "normal_shutdown_proof",
        ],
        "原 RustFS 重启事故证据",
    )?;
    let redis = &incident["redis_incident"];
    document::exact(
        redis,
        &[
            "kind",
            "failure",
            "runtime",
            "supervisor",
            "process",
            "port",
            "normal_job_exit_proof",
        ],
        "原 Windows Redis 重启事故证据",
    )?;
    if outer["operation"] != "recover"
        || outer["status"] != "external_termination_reconciled"
        || proof["format_version"] != 3
        || proof["kind"] != "reference-fixture-external-termination"
        || !proof["normal_shutdown_proof"].is_null()
        || !observation["normal_shutdown_proof"].is_null()
        || observation["status"] != "external-termination-unreconciled"
        || !rustfs["normal_shutdown_proof"].is_null()
        || redis["kind"] != "windows-redis-reboot-incident"
        || !redis["normal_job_exit_proof"].is_null()
        || redis["runtime"] != *second_runtime
    {
        return Err("第三次重启缺少同一第二代的 Windows 重启事故证据".into());
    }
    Ok(incident)
}

fn check_link(
    initial: &Value,
    previous: &Value,
    second: &Restart<'_>,
    third: &Restart<'_>,
    row: &Value,
    incident: &Value,
) -> Result<()> {
    let runtime = &third.generation["redis"]["runtime"];
    let redis = &incident["redis_incident"];
    let incident_number = row["number"].as_u64().ok_or("原事故恢复缺少阶段序号")?;
    let mut request = second.request.clone();
    let request_fields = request.as_object_mut().ok_or("原第二代 Redis 请求无效")?;
    for field in ["previous_identity", "previous_run_id"] {
        request_fields.insert(field.into(), third.request[field].clone());
    }
    if third.generation["previous_generation"] != *second.result
        || second.generation["attempt"]
            .as_u64()
            .is_none_or(|number| number >= incident_number)
        || third.generation["attempt"]
            .as_u64()
            .is_none_or(|number| number <= incident_number)
        || *previous != second.generation["redis"]["runtime"]
        || redis["supervisor"] != previous["supervisor"]
        || redis["process"] != previous["process"]
        || redis["port"] != previous["redis"]["port"]
        || third.request != request
    {
        return Err("第三次重启未继承第二代原请求、配置和事故身份".into());
    }
    check_redis_successor(initial, &runtime["redis"], previous, &third.request)
}

fn protect_third(evidence: &mut Evidence<'_>, third: &Restart<'_>) -> Result<Value> {
    let rustfs = &third.generation["rustfs"];
    evidence.read(&rustfs["request"], "第三代 RustFS 请求")?;
    for name in ["tree", "process_receipt", "launch_receipt"] {
        evidence.read(&rustfs["storage"][name], "第三代 RustFS 运行收据")?;
    }
    let runtime = &third.generation["redis"]["runtime"];
    let output = Path::new(document::text(&runtime["output"], "第三代 Redis 运行目录")?);
    let binding = common::binding(&output.join("runtime.json"))?;
    if evidence.read(&binding, "第三代 Redis 运行收据")? != *runtime {
        return Err("第三代 Redis 运行收据与重启登记不同".into());
    }
    Ok(binding)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn third_restart_requires_unique_passed_recover_predecessor() {
        let result = json!({"path":"external-result"});
        let row = json!({"number":13,"stage":"fixture-services","mode":"recover",
            "status":"passed","result":result});
        assert_eq!(
            recover_row(std::slice::from_ref(&row), &result).unwrap(),
            &row
        );
        assert!(recover_row(&[row.clone(), row.clone()], &result).is_err());
        for (field, value) in [
            ("stage", json!("cache-target")),
            ("mode", json!("reconcile")),
            ("status", json!("failed")),
            ("result", json!({"path":"other-recover"})),
        ] {
            let mut other = row.clone();
            other[field] = value;
            assert!(recover_row(&[other], &result).is_err());
        }
    }

    #[test]
    fn third_restart_rejects_other_external_termination_shapes_and_runtime_bindings() {
        let runtime = json!({"path":"second-runtime"});
        let rustfs = json!({"tree":{},"ready":{},"product":{},"member_failure":{},
            "actor_failure":{},"diagnostic":{},"normal_shutdown_proof":null});
        let redis = json!({"kind":"windows-redis-reboot-incident","failure":{},
            "runtime":runtime,"supervisor":{},"process":{},"port":24370,"normal_job_exit_proof":null});
        let result = json!({"operation":"recover","status":"external_termination_reconciled"});
        let proof = json!({"format_version":3,"kind":"reference-fixture-external-termination",
            "normal_shutdown_proof":null,"observation":{"status":"external-termination-unreconciled",
            "normal_shutdown_proof":null,
            "evidence":{"host_reboot":{},"rustfs_incident":rustfs,"redis_incident":redis}}});
        assert!(check_proof(&result, &proof, &runtime).is_ok());
        for (pointer, value) in [
            ("/format_version", json!(2)),
            (
                "/observation/evidence/redis_incident/runtime/path",
                json!("first-runtime"),
            ),
            (
                "/observation/evidence/redis_incident/kind",
                json!("windows-redis-external-termination"),
            ),
            (
                "/observation/evidence/redis_incident/normal_job_exit_proof",
                json!({}),
            ),
            (
                "/observation/evidence/rustfs_incident/normal_shutdown_proof",
                json!({}),
            ),
        ] {
            let mut invalid = proof.clone();
            *invalid.pointer_mut(pointer).unwrap() = value;
            assert!(check_proof(&result, &invalid, &runtime).is_err());
        }
        let mut extra = proof;
        extra["observation"]["evidence"]["redis_external"] = json!({});
        assert!(check_proof(&result, &extra, &runtime).is_err());
    }

    #[test]
    fn third_restart_binds_validated_second_generation_and_frozen_request() {
        let initial = json!({"platform":"windows","port":24370,"configuration":"original",
            "pid":100,"started":"200","run_id":"initial"});
        let second_result = json!({"path":"second-restart"});
        let third_result = json!({"path":"third-restart"});
        let mut previous_redis = initial.clone();
        previous_redis["pid"] = json!(102);
        previous_redis["started"] = json!("202");
        previous_redis["run_id"] = json!("second");
        let previous = json!({"format_version":3,"process":{"pid":102,"started":"202"},
            "supervisor":{"pid":112},"redis":previous_redis});
        let second = Restart {
            result: &second_result,
            outer: Value::Null,
            generation: json!({"attempt":12,"redis":{"runtime":previous}}),
            receipt: Value::Null,
            request: json!({"format_version":3,"previous_run_id":"first",
                "previous_identity":{"pid":101},"port":24370,"configuration":"original"}),
        };
        let mut selected = initial.clone();
        selected["pid"] = json!(103);
        selected["started"] = json!("203");
        selected["run_id"] = json!("third");
        let mut third = Restart {
            result: &third_result,
            outer: Value::Null,
            generation: json!({"attempt":14,"previous_generation":second_result,
                "redis":{"runtime":{"redis":selected}}}),
            receipt: Value::Null,
            request: json!({"format_version":3,"previous_run_id":"second",
                "previous_identity":previous["process"],"port":24370,"configuration":"original"}),
        };
        let incident = json!({"redis_incident":{"supervisor":previous["supervisor"],
            "process":previous["process"],"port":24370}});
        let row = json!({"number":13});
        let check =
            |third: &Restart<'_>| check_link(&initial, &previous, &second, third, &row, &incident);
        check(&third).unwrap();
        for (pointer, value) in [
            ("/attempt", json!(13)),
            ("/previous_generation/path", json!("first-restart")),
            ("/redis/runtime/redis/port", json!(24371)),
            ("/redis/runtime/redis/started", json!("202")),
            ("/redis/runtime/redis/run_id", json!("second")),
        ] {
            let entry = third.generation.pointer_mut(pointer).unwrap();
            let before = std::mem::replace(entry, value);
            assert!(check(&third).is_err());
            *third.generation.pointer_mut(pointer).unwrap() = before;
        }
        third.request["configuration"] = json!("other");
        assert!(check(&third).is_err());
    }
}

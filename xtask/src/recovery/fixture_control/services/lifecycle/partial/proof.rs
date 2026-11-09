use super::*;
use history::evidence::{bound, exact, members, path_is};

pub(super) fn create(
    context: &Context,
    services: &Value,
    output: &Path,
    completion: Value,
) -> Result<Value> {
    let mut states = json!({});
    for role in ["rustfs", "redis"] {
        states[role] = json!(if services["not_started"].get(role).is_some() {
            "not_started"
        } else {
            "stopped"
        });
    }
    let rustfs = if services["early_storage"].is_object() {
        json!({"state":"stopped","early":services["early_storage"]})
    } else if services["tree"].is_null() {
        Value::Null
    } else {
        json!({"state":"stopped","tree":services["tree"],"completion":completion})
    };
    let redis = if services["windows_failed"].is_object() {
        binding(&output.join("redis-failed-closed.json"))?
    } else if services["runtime"].is_null() {
        Value::Null
    } else {
        binding(&output.join(if services["reconciliation"].is_object() {
            "redis-reconciled.json"
        } else {
            "stopped.json"
        }))?
    };
    Ok(
        json!({"format_version":1,"kind":"reference-fixture-partial-close","run":context.run,
        "initial_complete":false,"initial":services["initial"],"services":states,
        "bindings":services["bindings"],"not_started":services["not_started"],
        "rustfs":rustfs,"redis":redis,"remote_writes":0,"resources_deleted":false}),
    )
}

pub(in crate::recovery::fixture_control::services) fn validate(
    run: &Path,
    number: usize,
    item: &Value,
    result: &Value,
) -> Result<()> {
    check(
        result["initial_complete"] == false && result["status"] == "services_partially_closed",
        "部分关闭不得伪装完整就绪",
    )?;
    let file = run.join(format!("lifecycle-{number:04}/partial-closed.json"));
    check(
        path_is(&result["evidence"]["path"], &file),
        "部分关闭证明不属于本阶段",
    )?;
    let proof = bound(run, &result["evidence"])?;
    exact(
        &proof,
        &[
            "format_version",
            "kind",
            "run",
            "initial_complete",
            "initial",
            "services",
            "bindings",
            "not_started",
            "rustfs",
            "redis",
            "remote_writes",
            "resources_deleted",
        ],
    )?;
    let ledger = state::load(run, true)?;
    let attempts = state::attempts(&ledger)?;
    let initial = proof["initial"]
        .as_array()
        .ok_or("部分关闭缺少原始初代前缀")?;
    check(
        proof["format_version"] == 1
            && proof["kind"] == "reference-fixture-partial-close"
            && path_is(&proof["run"], run)
            && proof["initial_complete"] == false
            && proof["remote_writes"] == 0
            && proof["resources_deleted"] == false
            && initial.len() <= 2
            && number == initial.len() + 1
            && initial.len() <= attempts.len()
            && attempts[..initial.len()] == *initial
            && item["sources"]["state_before"].is_object()
            && proof["services"] == result["services"],
        "部分关闭未绑定完整失败前缀或零资源删除事实",
    )?;
    validate_inputs(run, &proof, initial)?;
    validate_roles(run, number, &proof)
}

fn validate_inputs(run: &Path, proof: &Value, initial: &[Value]) -> Result<()> {
    let mut expected = json!({});
    for (index, role) in ["rustfs", "redis"].iter().enumerate() {
        if let Some(attempt) = initial.get(index) {
            check(
                ["passed", "failed"].iter().any(|s| attempt["status"] == *s)
                    && (index == 0 || initial[0]["status"] == "passed"),
                "部分关闭初代前缀不合法",
            )?;
            let request = &attempt["sources"]["request"];
            check(
                path_is(&request["path"], &run.join(role).join("request.json")),
                "部分关闭冻结请求路径不同",
            )?;
            expected[format!("{role}_request")] = request.clone();
            expected[format!("{role}_controller")] =
                binding(&run.join(format!("controller-{:04}.json", index + 1)))?;
            if attempt["status"] == "passed" {
                check(
                    proof["not_started"].get(*role).is_none(),
                    "成功启动步骤不能标为未启动",
                )?;
                expected[format!("{role}_result")] = attempt["result"].clone();
            }
            if proof["not_started"].get(*role).is_some() {
                check(
                    proof["not_started"][*role]["request"] == *request,
                    "未启动证明未绑定原请求",
                )?;
            }
        } else {
            check(
                proof["not_started"].get(*role).is_some()
                    && proof["not_started"][*role]["request"].is_null(),
                "缺少未执行步骤的零启动证明",
            )?;
        }
    }
    storage_bindings(run, proof, initial, &mut expected)?;
    if !proof["redis"].is_null() {
        if let Some(validated) = failed_cache(run, proof)? {
            expected
                .as_object_mut()
                .ok_or("部分关闭输入集合无效")?
                .extend(
                    validated["bindings"]
                        .as_object()
                        .ok_or("失败清理绑定集合无效")?
                        .clone(),
                );
        } else {
            expected["redis_runtime"] = binding(&run.join("redis/runtime.json"))?;
        }
    }
    check(
        proof["bindings"] == expected,
        "部分关闭绑定集合缺失、变化或包含未知输入",
    )?;
    for descriptor in expected.as_object().ok_or("部分关闭输入集合无效")?.values() {
        bound(run, descriptor)?;
    }
    Ok(())
}

fn storage_bindings(
    run: &Path,
    proof: &Value,
    initial: &[Value],
    expected: &mut Value,
) -> Result<()> {
    if !proof["rustfs"].is_null() {
        if proof["rustfs"]["early"].is_object() {
            let closed = super::super::super::storage::closed::read(
                &crate::workspace::root_dir(),
                run,
                &initial[0],
            )?;
            check(
                closed == proof["rustfs"]["early"],
                "部分关闭未绑定原真实早期关闭证据",
            )?;
            expected
                .as_object_mut()
                .ok_or("部分关闭输入集合无效")?
                .extend(
                    closed["bindings"]
                        .as_object()
                        .ok_or("原早期关闭输入集合无效")?
                        .clone(),
                );
        } else {
            for file in [
                "intent.json",
                "process.json",
                "launch.json",
                "rustfs-tree.json",
            ] {
                expected[format!("rustfs_{file}")] = binding(&run.join("rustfs").join(file))?;
            }
            if run.join("rustfs/actor.json").try_exists()?
                || run.join("rustfs/actor-binding.json").try_exists()?
            {
                let records = super::super::super::storage::records::load(
                    &crate::workspace::root_dir(),
                    run,
                    &initial[0],
                )?;
                expected
                    .as_object_mut()
                    .ok_or("部分关闭输入集合无效")?
                    .extend(
                        records["bindings"]
                            .as_object()
                            .ok_or("原 actor 输入集合无效")?
                            .clone(),
                    );
            }
        }
    }
    Ok(())
}

fn failed_cache(run: &Path, proof: &Value) -> Result<Option<Value>> {
    if proof["initial"][1]["status"] != "failed" || proof["redis"].is_null() {
        return Ok(None);
    }
    check(
        proof["initial"][1]["result"].is_null(),
        "失败首代不能含发布结果",
    )?;
    let request = bound(run, &proof["bindings"]["redis_request"])?;
    Ok(Some(
        crate::recovery::clone::native::cache::failed_cleanup::validate(
            &request,
            &run.join("redis"),
        )?,
    ))
}

fn validate_roles(run: &Path, number: usize, proof: &Value) -> Result<()> {
    exact(&proof["services"], &["rustfs", "redis"])?;
    let absent = proof["not_started"]
        .as_object()
        .ok_or("部分关闭缺少未启动证据")?;
    check(
        absent.contains_key("buckets")
            && absent
                .keys()
                .all(|key| ["rustfs", "redis", "buckets"].contains(&key.as_str())),
        "部分关闭未排除对象桶写入",
    )?;
    for (role, value) in absent {
        exact(value, &["request"])?;
        inputs::verify_absent(
            &run.join(role),
            (!value["request"].is_null()).then_some(&value["request"]),
        )?;
    }
    for role in ["rustfs", "redis"] {
        if absent.contains_key(role) {
            check(
                proof["services"][role] == "not_started" && proof[role].is_null(),
                "未启动角色不能伪造退出收据",
            )?;
        } else if role == "rustfs" {
            if proof["rustfs"]["early"].is_object() {
                exact(&proof["rustfs"], &["state", "early"])?;
                check(
                    proof["services"][role] == "stopped" && proof["rustfs"]["state"] == "stopped",
                    "早期关闭角色必须明确已停止",
                )?;
                super::super::super::storage::closed::recheck(&proof["rustfs"]["early"])?;
                continue;
            }
            exact(&proof["rustfs"], &["state", "tree", "completion"])?;
            check(
                proof["services"][role] == "stopped"
                    && proof["rustfs"]["tree"]
                        == bound(run, &proof["bindings"]["rustfs_rustfs-tree.json"])?,
                "部分关闭进程树不属于原始绑定",
            )?;
            members(&proof["rustfs"], run)?;
        } else {
            if let Some(validated) = failed_cache(run, proof)? {
                if let Some(closed) = validated.get("windows_closed") {
                    check(
                        proof["services"][role] == "stopped"
                            && path_is(
                                &proof["redis"]["path"],
                                &run.join(format!(
                                    "lifecycle-{number:04}/redis-failed-closed.json"
                                )),
                            )
                            && bound(run, &proof["redis"])? == *closed,
                        "Windows Redis 部分关闭未绑定原失败和退出证据",
                    )?;
                    continue;
                }
                if validated["reconcile_required"] == true {
                    check(
                        proof["services"][role] == "stopped",
                        "协调角色必须确认为已退出",
                    )?;
                    reconciliation::validate(run, number, &proof["redis"], &validated)?;
                    continue;
                }
            }
            let expected = run.join(format!("lifecycle-{number:04}/stopped.json"));
            let stopped = bound(run, &proof["redis"])?;
            let original = if let Some(validated) = failed_cache(run, proof)? {
                check(
                    stopped["already_stopped"] == true,
                    "失败清理必须注明原代次已经停止",
                )?;
                validated["runtime"].clone()
            } else {
                bound(run, &proof["bindings"]["redis_runtime"])?
            };
            check(
                path_is(&proof["redis"]["path"], &expected)
                    && proof["services"][role] == "stopped"
                    && stopped["status"] == "redis_process_stopped"
                    && stopped["resources_deleted"] == false
                    && stopped["runtime"] == original,
                "Redis 部分关闭未证明原登记身份退出",
            )?;
            if original["format_version"] == 3 {
                super::super::super::history::evidence::windows_redis_stop(run, &stopped)?;
            } else {
                check(
                    stopped["alive"] == false,
                    "Redis 部分关闭未证明原登记身份退出",
                )?;
            }
        }
    }
    Ok(())
}

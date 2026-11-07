use super::*;

pub(super) fn run(
    runtime: &mut dyn Runtime,
    context: &Context,
    owner_file: &Path,
) -> Result<Value> {
    let history = context.history(true)?;
    context.require_current_owner(owner_file)?;
    check(
        !history["restart_predecessor"].is_null()
            && history["unsettled"].as_array().is_some_and(Vec::is_empty),
        "服务重启必须紧接已完成且唯一的正常关闭或外部终止核对",
    )?;
    context.no_unknown()?;
    check(
        controller(runtime, context)?.is_null(),
        "服务重启前仍存在控制器",
    )?;
    let previous = registered(runtime, context, true)?;
    let stopped = observe(runtime, &previous)?;
    if history["closed"] == true {
        check(
            stopped == expected_stopped(),
            "重启前正常关闭的原服务代次状态变化",
        )?;
    } else {
        check(
            !stopped["termination"].is_null(),
            "重启前原服务代次不是已核对的外部终止状态",
        )?;
    }
    let _guard = ProcessGuard::acquire(&context.run)?;
    owned(context, |owner| {
        context.guard(None)?;
        check_observation(runtime, &previous, &stopped, "重启前原服务状态变化")?;
        let mut sources = context.sources();
        sources["restart_predecessor"] = history["restart_predecessor"].clone();
        let expected = state::attempts(&context.value["state"])?.len() as u64 + 1;
        stage(
            context,
            "restart",
            &sources,
            owner,
            |number, controller, running, output| {
                check(number == expected, "夹具服务重启代次编号与账本不一致")?;
                restart_generation(
                    runtime,
                    context,
                    RestartAttempt {
                        history: &history,
                        previous: &previous,
                        stopped: &stopped,
                    },
                    number,
                    controller,
                    running,
                    output,
                )
            },
        )
    })
}

struct RestartAttempt<'a> {
    history: &'a Value,
    previous: &'a Value,
    stopped: &'a Value,
}

fn restart_generation(
    runtime: &mut dyn Runtime,
    context: &Context,
    attempt: RestartAttempt<'_>,
    number: u64,
    controller: &Value,
    running: &Value,
    output: &Path,
) -> Result<Value> {
    new_output(context, output)?;
    let mut storage = None;
    let mut cache = None;
    let mut requests = attempt.previous["requests"].clone();
    let attempt = (|| {
        requests = restart_requests(attempt.previous, output)?;
        context.guard(Some(running))?;
        check_observation(
            runtime,
            attempt.previous,
            attempt.stopped,
            "RustFS 重启前原服务状态变化",
        )?;
        storage = Some(runtime.call("storage-start",json!({"request":requests["rustfs"],"private":context.value["private"],
                    "output":output.join("rustfs"),"manifest":context.value["sources"]["manifest"],"controller":controller,
                    "number":number,"context":context.value,"running":running}))?);
        context.guard(Some(running))?;
        cache = Some(runtime.call(
            "cache-start",
            json!({"request":requests["redis"],"private":context.value["private"],
                    "output":output.join("redis"),"context":context.value,"running":running}),
        )?);
        publish_generation(
            runtime,
            context,
            Generation {
                history: attempt.history,
                previous: attempt.previous,
                requests: &requests,
                storage: storage.as_ref().ok_or("缺少 RustFS 结果")?,
                cache: cache.as_ref().ok_or("缺少 Redis 结果")?,
            },
            number,
            controller,
            running,
            output,
        )
    })();
    match attempt {
        Ok(value) => Ok(value),
        Err(error) => {
            let cleanup = cleanup(
                runtime,
                context,
                &requests,
                storage.as_ref(),
                cache.as_ref(),
                output,
            );
            if cleanup.is_err() {
                return Err(Box::new(model::LifecycleError {
                    category: "RestartCleanupUnknown",
                    message: "服务重启失败且精确回收未确认；保留原失败与全部现场，禁止自动重放",
                }));
            }
            Err(error)
        }
    }
}

pub(super) fn restart_requests(services: &Value, output: &Path) -> Result<Value> {
    let mut requests = services["requests"].clone();
    if !services["cache_effective"].is_null() {
        requests["redis"] = services["cache_effective"].clone();
    }
    let previous = &services["runtime"];
    if requests["redis"]["format_version"] == 3 || previous["format_version"] == 3 {
        check(
            requests["redis"]["format_version"] == 3
                && previous["format_version"] == 3
                && requests["redis"].get("previous_boot_id").is_none(),
            "Windows Redis 重启请求与登记代次平台不同或混有 Linux 字段",
        )?;
        crate::recovery::clone::native::resources::common::validate_process_identity(
            &previous["process"],
        )?;
        requests["redis"]["previous_identity"] = previous["process"].clone();
    } else {
        requests["redis"]["previous_identity"] = json!({"pid":previous["linux_identity"]["pid"],
            "started":previous["linux_identity"]["started"],"executable":previous["linux_identity"]["executable"]});
        requests["redis"]["previous_boot_id"] = previous["linux_identity"]["boot_id"].clone();
    }
    requests["redis"]["previous_run_id"] = previous["redis"]["run_id"].clone();
    for role in ["rustfs", "redis"] {
        std::fs::create_dir(output.join(role))?;
        json_io::write_new(&output.join(role).join("request.json"), &requests[role])?;
    }
    Ok(requests)
}

struct Generation<'a> {
    history: &'a Value,
    previous: &'a Value,
    requests: &'a Value,
    storage: &'a Value,
    cache: &'a Value,
}

fn publish_generation(
    runtime: &mut dyn Runtime,
    context: &Context,
    parts: Generation<'_>,
    number: u64,
    controller: &Value,
    running: &Value,
    output: &Path,
) -> Result<Value> {
    let tree = runtime.call(
        "tree-read",
        json!({"output":output.join("rustfs"),"scope_id":parts.requests["rustfs"]["scope_id"]}),
    )?;
    let current = json!({"requests":parts.requests,"tree":tree,"runtime":parts.cache,"storage":parts.storage,
        "storage_observation":null,"origin":parts.previous["origin"],
        "evidence":{"rustfs_tree":parts.storage["tree"],"redis_runtime":binding(&output.join("redis/runtime.json"))?}});
    check_observation(
        runtime,
        &current,
        &expected_running(),
        "夹具服务新代次未同时就绪",
    )?;
    let file = output.join("restart.json");
    let generation = json!({"format_version":1,"kind":"reference-fixture-service-generation","run":context.run,
        "attempt":number,"owner":context.value["sources"]["state_before"],"predecessor":parts.history["restart_predecessor"],
        "previous_generation":parts.history["active_generation"],"controller":controller,"services":{"redis":"running","rustfs":"running"},
        "rustfs":{"request":binding(&output.join("rustfs/request.json"))?,"storage":parts.storage},
        "redis":{"request":binding(&output.join("redis/request.json"))?,"runtime":parts.cache},"remote_writes":0,"resources_deleted":false});
    json_io::write_new(&file, &generation)?;
    context.guard(Some(running))?;
    check_observation(
        runtime,
        &current,
        &expected_running(),
        "发布重启代次前服务状态变化",
    )?;
    result(
        context,
        "restart",
        "services_restarted",
        json!({"controller":controller,"services":generation["services"],
        "generation":binding(&file)?,"previous_generation":parts.history["active_generation"],"next_operation":"close"}),
    )
}

fn cleanup(
    runtime: &mut dyn Runtime,
    context: &Context,
    requests: &Value,
    storage: Option<&Value>,
    cache: Option<&Value>,
    output: &Path,
) -> Result<()> {
    if !output.exists() {
        return Ok(());
    }
    let directory = output.join("failed-start-cleanup");
    std::fs::create_dir(&directory)?;
    let mut failed = false;
    if let Some(cache) = cache {
        failed |= runtime.call("cache-stop",json!({"request":requests["redis"],"private":context.value["private"],"runtime":cache,"output":directory})).is_err();
    }
    if storage.is_some() {
        let stopped = (|| {
            let tree = runtime.call(
                "tree-read",
                json!({"output":output.join("rustfs"),"scope_id":requests["rustfs"]["scope_id"]}),
            )?;
            runtime.call("tree-stop", json!({"tree":tree,"crash":true}))?;
            runtime.call("completion", json!({"tree":tree}))
        })();
        failed |= stopped.is_err();
    }
    check(!failed, "夹具服务重启失败后的精确回收未完成")
}

#[cfg(test)]
#[test]
fn restart_preserves_the_registered_platform_predecessor_in_each_request() {
    let root = crate::workspace::root_dir();
    let base = root.join(".local-tests");
    std::fs::create_dir_all(&base).unwrap();
    let directory = tempfile::tempdir_in(base).unwrap();
    let process = json!({"pid":36784,"started":"134000000000000000",
        "executable":root.join("redis-server.exe")});
    let linux = json!({"pid":17176,"started":"8823054","executable":"/usr/bin/redis-server"});
    let boot = "5a81dc04-9e9a-416b-8eab-1b059bddf489";
    let run_id = "a".repeat(40);
    for (name, redis, previous, expected, expected_boot) in [
        (
            "windows",
            json!({"format_version":3,"previous_identity":null,"previous_run_id":null}),
            json!({"format_version":3,"process":process,"redis":{"run_id":run_id}}),
            process.clone(),
            None,
        ),
        (
            "linux",
            json!({"format_version":2,"previous_identity":null,"previous_run_id":null,"previous_boot_id":null}),
            json!({"format_version":2,"linux_identity":{"pid":linux["pid"],"started":linux["started"],
                "executable":linux["executable"],"boot_id":boot},"redis":{"run_id":run_id}}),
            linux,
            Some(json!(boot)),
        ),
    ] {
        let output = directory.path().join(name);
        std::fs::create_dir(&output).unwrap();
        let services = json!({"requests":{"rustfs":{},"redis":redis},"runtime":previous});
        let request = restart_requests(&services, &output).unwrap()["redis"].clone();
        assert_eq!(request["previous_identity"], expected);
        assert_eq!(request["previous_run_id"], run_id);
        assert_eq!(request.get("previous_boot_id"), expected_boot.as_ref());
        assert_eq!(read(&output.join("redis/request.json")).unwrap(), request);
    }
}

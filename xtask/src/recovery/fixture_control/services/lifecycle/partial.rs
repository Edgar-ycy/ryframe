//! 首代只允许核验后关闭实际启动者；保留失败证据，不能成为续建或重启前驱。
use super::*;
use crate::recovery::clone::native::resources::common;

#[path = "partial/inputs.rs"]
mod inputs;
#[path = "partial/proof.rs"]
pub(in crate::recovery::fixture_control::services) mod proof;
#[path = "partial/reconciliation.rs"]
mod reconciliation;

pub(super) fn close(
    runtime: &mut dyn Runtime,
    context: &Context,
    history: &Value,
) -> Result<Value> {
    context.no_unknown()?;
    check(
        controller(runtime, context)?.is_null(),
        "部分关闭前仍有当前或遗留控制器",
    )?;
    check(
        history["unsettled"].as_array().is_some_and(Vec::is_empty),
        "部分关闭有未确认结果，禁止重放",
    )?;
    let services = inputs::load(runtime, context)?;
    if history["closed"] == true {
        stopped(runtime, &services)?;
        return result(
            context,
            "close",
            "services_already_partially_closed",
            json!({"initial_complete":false}),
        );
    }
    ready_to_close(runtime, &services)?;
    let _guard = ProcessGuard::acquire(&context.run)?;
    owned(context, |owner| {
        stable(runtime, context, &services, None)?;
        stage(
            context,
            "close",
            &context.sources(),
            owner,
            |_, controller, current, output| {
                stop(runtime, context, &services, controller, current, output)
            },
        )
    })
}

fn stop(
    runtime: &mut dyn Runtime,
    context: &Context,
    services: &Value,
    controller: &Value,
    current: &Value,
    output: &Path,
) -> Result<Value> {
    new_output(context, output)?;
    stable(runtime, context, services, Some(current))?;
    if !services["runtime"].is_null() {
        let cache = close_cache(runtime, context, services, output, current)?;
        check(
            cache["status"] == "redis_process_stopped",
            "部分关闭未确认 Redis 退出",
        )?;
    }
    stable(runtime, context, services, Some(current))?;
    if !services["tree"].is_null() {
        runtime.call("tree-stop", json!({"tree":services["tree"],"crash":false}))?;
    }
    stopped(runtime, services)?;
    let completion = if services["tree"].is_null() {
        Value::Null
    } else {
        runtime.call("completion", json!({"tree":services["tree"]}))?
    };
    stable(runtime, context, services, Some(current))?;
    let file = output.join("partial-closed.json");
    if services["reconciliation"].is_object() {
        reconciliation::publish(runtime, context, services, output)?;
    }
    let receipt = proof::create(context, services, output, completion)?;
    json_io::write_new(&file, &receipt)?;
    let result = result(
        context,
        "close",
        "services_partially_closed",
        json!({"initial_complete":false,
                "controller":controller,"services":receipt["services"],"evidence":binding(&file)?}),
    )?;
    let ledger = state::load(&context.run, true)?;
    let attempt = state::attempts(&ledger)?
        .last()
        .ok_or("部分关闭缺少当前阶段")?;
    let number = usize::try_from(attempt["number"].as_u64().ok_or("部分关闭阶段编号无效")?)?;
    proof::validate(&context.run, number, attempt, &result)?;
    Ok(result)
}

fn close_cache(
    runtime: &mut dyn Runtime,
    context: &Context,
    services: &Value,
    output: &Path,
    current: &Value,
) -> Result<Value> {
    if services["reconciliation"].is_object() {
        reconciliation::observe(runtime, services)?;
        return Ok(json!({"status":"redis_process_stopped"}));
    }
    let state = cache_state(runtime, services, None)?;
    check(
        services["failed_cleanup"] != true || state == "stopped",
        "失败清理的原 Redis 必须保持停止",
    )?;
    if state == "stopped" {
        let receipt = json!({"status":"redis_process_stopped","already_stopped":true,
            "alive":false,"resources_deleted":false,"runtime":services["runtime"]});
        json_io::write_new(&output.join("stopped.json"), &receipt)?;
        return Ok(receipt);
    }
    runtime.call(
        "cache-stop",
        json!({"request":services["requests"]["redis"],
        "private":context.value["private"],"runtime":services["runtime"],"output":output,
        "context":context.value,"running":current}),
    )
}

fn cache_state(
    runtime: &mut dyn Runtime,
    services: &Value,
    expected: Option<&str>,
) -> Result<&'static str> {
    if services["reconciliation"].is_object() {
        check(expected != Some("running"), "协调不能要求或声明启动成功")?;
        reconciliation::observe(runtime, services)?;
        return Ok("stopped");
    }
    let observed = runtime.call(
        "cache-status",
        json!({"request":services["requests"]["redis"],
        "runtime":services["runtime"],"effective":null}),
    )?;
    let state = match observed["state"].as_str() {
        Some("running") => "running",
        Some("stopped") => "stopped",
        _ => return Err(model::error("部分关闭 Redis 原代次状态未知，保留现场")),
    };
    check(
        expected.is_none_or(|expected| state == expected),
        "部分关闭 Redis 状态与原代次要求不同",
    )?;
    Ok(state)
}

fn stable(
    runtime: &mut dyn Runtime,
    context: &Context,
    services: &Value,
    current: Option<&Value>,
) -> Result<()> {
    context.guard(current)?;
    context.no_unknown()?;
    check(
        inputs::load(runtime, context)? == *services,
        "部分关闭期间初代证据、请求或未启动事实变化",
    )
}

fn ready_to_close(runtime: &mut dyn Runtime, services: &Value) -> Result<&'static str> {
    let cache = if !services["runtime"].is_null() {
        cache_state(
            runtime,
            services,
            if services["failed_cleanup"] == true {
                Some("stopped")
            } else {
                None
            },
        )?
    } else {
        "not_started"
    };
    if !services["tree"].is_null() {
        let tree = &services["tree"];
        let directory = Path::new(text(tree, "runtime_directory")?);
        let operation = text(tree, "operation_id")?;
        for filename in [
            format!("rustfs-tree-{operation}-control.json"),
            format!("rustfs-tree-{operation}-result.json"),
            format!("rustfs-members-{operation}-stopped.json"),
        ] {
            check(
                !directory.join(filename).try_exists()?,
                "部分关闭前已有停止或未知控制结果，禁止重放",
            )?;
        }
        check(
            observation::identities(runtime, tree)?
                == json!({"supervisor":"running","monitor":"running","process":"running"}),
            "部分关闭前 RustFS 完整身份无法确认；禁止终止未知进程",
        )?;
        check(
            runtime.call(
                "storage-running",
                json!({"identity":tree["process"],"request":services["requests"]["rustfs"]}),
            )? == true,
            "部分关闭前 RustFS 产品身份已变化",
        )?;
    }
    Ok(cache)
}

fn stopped(runtime: &mut dyn Runtime, services: &Value) -> Result<()> {
    if !services["runtime"].is_null() {
        cache_state(runtime, services, Some("stopped"))?;
    }
    if !services["tree"].is_null() {
        check(
            observation::identities(runtime, &services["tree"])?
                == json!({"supervisor":"missing","monitor":"missing","process":"missing"}),
            "部分关闭后 RustFS 仍有存活或未知成员",
        )?;
        runtime.call("members-wait", json!({"tree":services["tree"]}))?;
        observation::closed(&services["requests"]["rustfs"])?;
    }
    Ok(())
}

pub(super) fn verify_closed(runtime: &mut dyn Runtime, context: &Context) -> Result<Value> {
    let services = inputs::load(runtime, context)?;
    stopped(runtime, &services)?;
    let mut states = json!({});
    for role in ["rustfs", "redis"] {
        states[role] = json!(if services["not_started"].get(role).is_some() {
            "not_started"
        } else {
            "stopped"
        });
    }
    Ok(states)
}

pub(super) fn can_close(runtime: &mut dyn Runtime, context: &Context) -> Result<&'static str> {
    let services = inputs::load(runtime, context)?;
    ready_to_close(runtime, &services)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct CacheRuntime {
        state: &'static str,
        calls: Vec<String>,
    }
    impl Runtime for CacheRuntime {
        fn now(&self) -> Duration {
            Duration::ZERO
        }
        fn pause(&mut self, _: Duration) {
            panic!("只读核验不等待控制")
        }
        fn call(&mut self, operation: &str, _: Value) -> Result<Value> {
            self.calls.push(operation.into());
            check(operation == "cache-status", "不得重发停止或启动控制")?;
            Ok(json!({"state":self.state}))
        }
    }

    #[test]
    fn failed_cleanup_requires_stopped_and_writes_only_new_stage_receipt_without_control() {
        let backend = crate::workspace::root_dir();
        let base = backend.join(".local-tests");
        std::fs::create_dir_all(&base).unwrap();
        let temporary = tempfile::tempdir_in(base).unwrap();
        let context = Context {
            backend,
            run: temporary.path().to_path_buf(),
            value: json!({}),
        };
        for state in ["stopped", "running", "unknown"] {
            let output = temporary.path().join(state);
            std::fs::create_dir(&output).unwrap();
            let mut runtime = CacheRuntime {
                state,
                calls: Vec::new(),
            };
            let services = json!({"failed_cleanup":true,"requests":{"redis":{}},
                "runtime":{"original":true},"tree":null});
            assert_eq!(
                ready_to_close(&mut runtime, &services).is_ok(),
                state == "stopped"
            );
            let result = close_cache(&mut runtime, &context, &services, &output, &Value::Null);
            assert_eq!(result.is_ok(), state == "stopped");
            assert_eq!(runtime.calls, ["cache-status", "cache-status"]);
            if state == "stopped" {
                let receipt = read(&output.join("stopped.json")).unwrap();
                assert_eq!(receipt["runtime"], services["runtime"]);
                assert_eq!(receipt["already_stopped"], true);
                assert_eq!(receipt["alive"], false);
                assert_eq!(receipt["resources_deleted"], false);
            } else {
                assert!(!output.join("stopped.json").exists());
            }
            assert!(!output.join("runtime.json").exists());
            assert!(!output.join("partial.json").exists());
        }
        let services = json!({"requests":{"redis":{}},"runtime":{},"tree":null});
        for state in ["running", "stopped", "unknown"] {
            let mut runtime = CacheRuntime {
                state,
                calls: Vec::new(),
            };
            assert_eq!(
                ready_to_close(&mut runtime, &services).is_ok(),
                state != "unknown"
            );
            assert_eq!(runtime.calls, ["cache-status"]);
        }
    }
}

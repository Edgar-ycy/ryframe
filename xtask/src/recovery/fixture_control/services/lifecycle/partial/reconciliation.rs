//! 仅在新阶段登记只读协调结果，不补造原停止或启动收据。
use super::*;
use crate::recovery::clone::native::cache::failed_cleanup;
use history::evidence::exact;

pub(super) fn observe(runtime: &mut dyn Runtime, services: &Value) -> Result<Value> {
    let value = runtime.call(
        "cache-status",
        json!({"request":services["requests"]["redis"],
        "runtime":services["runtime"],"effective":null}),
    )?;
    observation(&value, &services["runtime"])?;
    Ok(value)
}

fn observation(value: &Value, original: &Value) -> Result<()> {
    exact(value, &["state", "launcher_alive", "linux"])?;
    exact(
        &value["linux"],
        &["identity", "alive", "boot_id", "terminated"],
    )?;
    check(
        value["state"] == "stopped"
            && value["launcher_alive"] == false
            && value["linux"]["alive"] == false
            && value["linux"]["terminated"] == false
            && value["linux"]["identity"] == original["linux_identity"]
            && value["linux"]["boot_id"]
                .as_str()
                .is_some_and(|v| uuid::Uuid::parse_str(v).is_ok()),
        "只读协调未证明原identity、launcher退出及原生端口关闭",
    )
}

pub(super) fn publish(
    runtime: &mut dyn Runtime,
    context: &Context,
    services: &Value,
    output: &Path,
) -> Result<()> {
    let request = &services["requests"]["redis"];
    let before = failed_cleanup::validate(request, &context.run.join("redis"))?;
    check(before == services["reconciliation"], "协调前原失败证据变化")?;
    let observed = observe(runtime, services)?;
    check(
        failed_cleanup::validate(request, &context.run.join("redis"))? == before,
        "协调观察期间原失败证据变化",
    )?;
    json_io::write_new(
        &output.join("redis-reconciled.json"),
        &json!({
        "format_version":1,"kind":"redis-failure-reconciliation","candidate":before,
        "observation":observed,"remote_writes":0,"resources_deleted":false}),
    )
}

pub(super) fn validate(
    run: &Path,
    number: usize,
    descriptor: &Value,
    candidate: &Value,
) -> Result<()> {
    use history::evidence::{bound, path_is};
    check(
        path_is(
            &descriptor["path"],
            &run.join(format!("lifecycle-{number:04}/redis-reconciled.json")),
        ),
        "协调收据不属于当前关闭阶段",
    )?;
    let value = bound(run, descriptor)?;
    exact(
        &value,
        &[
            "format_version",
            "kind",
            "candidate",
            "observation",
            "remote_writes",
            "resources_deleted",
        ],
    )?;
    check(
        value["format_version"] == 1
            && value["kind"] == "redis-failure-reconciliation"
            && value["candidate"] == *candidate
            && candidate["reconcile_required"] == true
            && value["remote_writes"] == 0
            && value["resources_deleted"] == false,
        "协调收据未绑定原失败前后像或只读事实",
    )?;
    observation(&value["observation"], &candidate["runtime"])
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Readonly {
        value: Value,
        calls: Vec<String>,
    }
    impl Runtime for Readonly {
        fn now(&self) -> Duration {
            Duration::ZERO
        }
        fn pause(&mut self, _: Duration) {
            panic!("只读协调不轮询或控制服务")
        }
        fn call(&mut self, operation: &str, _: Value) -> Result<Value> {
            self.calls.push(operation.into());
            check(operation == "cache-status", "协调不得发送停止或启动命令")?;
            Ok(self.value.clone())
        }
    }
    fn values() -> (Value, Value) {
        let original = json!({"linux_identity":{"pid":17176,"started":"8823054",
            "executable":"/usr/bin/redis-check-rdb","boot_id":"66cd0551-5c44-4dbb-814d-73629557e798"}});
        let observed = json!({"state":"stopped","launcher_alive":false,
            "linux":{"identity":original["linux_identity"],"alive":false,"terminated":false,
            "boot_id":original["linux_identity"]["boot_id"]}});
        (original, observed)
    }
    #[test]
    fn reconciliation_requires_fresh_exact_readonly_exit_observation_without_stop() {
        let (original, observed) = values();
        let services = json!({"runtime":original,"requests":{"redis":{}}});
        let mut runtime = Readonly {
            value: observed.clone(),
            calls: Vec::new(),
        };
        assert_eq!(observe(&mut runtime, &services).unwrap(), observed);
        for (pointer, value) in [
            ("/state", json!("unknown")),
            ("/launcher_alive", json!(true)),
            ("/linux/alive", json!(true)),
            ("/linux/terminated", json!(true)),
            ("/linux/identity/started", json!("different")),
            ("/linux/boot_id", json!("invalid")),
        ] {
            runtime.value = observed.clone();
            *runtime.value.pointer_mut(pointer).unwrap() = value;
            assert!(observe(&mut runtime, &services).is_err());
        }
        assert!(runtime.calls.iter().all(|v| v == "cache-status"));
    }
    #[test]
    fn reconciliation_receipt_is_bound_to_new_stage_and_original_candidate() {
        let base = crate::workspace::root_dir().join(".local-tests");
        std::fs::create_dir_all(&base).unwrap();
        let temporary = tempfile::tempdir_in(base).unwrap();
        let run = temporary.path();
        let directory = run.join("lifecycle-0003");
        std::fs::create_dir(&directory).unwrap();
        let (original, observed) = values();
        let candidate = json!({"runtime":original,"reconcile_required":true,"bindings":{}});
        let value = json!({"format_version":1,"kind":"redis-failure-reconciliation","candidate":candidate,
            "observation":observed,"remote_writes":0,"resources_deleted":false});
        let path = directory.join("redis-reconciled.json");
        json_io::write_new(&path, &value).unwrap();
        validate(run, 3, &binding(&path).unwrap(), &candidate).unwrap();
        assert!(validate(run, 4, &binding(&path).unwrap(), &candidate).is_err());
        let descriptor = binding(&path).unwrap();
        let mut changed = value;
        changed["candidate"]["runtime"]["linux_identity"]["started"] = json!("different");
        json_io::write_replace(&path, &changed).unwrap();
        assert!(validate(run, 3, &descriptor, &candidate).is_err());
        assert!(validate(run, 3, &binding(&path).unwrap(), &candidate).is_err());
        assert!(!directory.join("stopped.json").exists());
        assert!(!run.join("redis/runtime.json").exists());
    }
}

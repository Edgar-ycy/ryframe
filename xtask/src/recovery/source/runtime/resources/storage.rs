//! 从原复制账本取得存储代次，再观察实际二进制、内核、argv、监听与 Redis 配置。
use super::{Resources, Transport, document};
use crate::{
    Result,
    cli::CloneSide,
    recovery::clone::native::{
        cache, digest,
        lifecycle::manifest::fixture_storage::RustfsHandoff,
        resources::{common, storage::registered},
        target_verify::storage::{SystemObserver, kernel, redis_layout, rustfs},
    },
};
use serde_json::Value;
use std::path::Path;

impl<T: Transport> Resources<'_, T> {
    pub(crate) fn storage_identity(&self, run: &Path) -> Result<Value> {
        (self.checkpoint)()?;
        redis_layout::require_windows(&self.request["storage"]["redis"])?;
        let state = common::binding(&run.join("state.json"))?;
        let manifest = document::read(&run.join("manifest.json"))?;
        if manifest["copy_stage"] != "source_to_seed" {
            return Err("来源存储观察只消费原 source_to_seed 复制账本".into());
        }
        let (runtime, handoff) =
            registered::registered_with_handoff(self.backend, run, CloneSide::Target, None)?
                .map_or((None, None), |(runtime, handoff)| (Some(runtime), handoff));
        let cache = cache::binding::load(self.backend, run, None)?;
        let mut storage = self.request["storage"].clone();
        if let Some(runtime) = &runtime {
            let request = common::read_bound(self.backend, &runtime["request"], "源存储重启请求")?;
            for name in ["identity", "process_receipt", "launch_receipt"] {
                if request["previous"][name] != self.request["storage"]["rustfs"][name] {
                    return Err("源存储重启没有绑定原目标完整 RustFS 收据".into());
                }
            }
            storage["rustfs"] = runtime["storage"].clone();
        }
        let original_redis = &self.request["storage"]["redis"];
        let cache_redis = if cache.is_null() {
            None
        } else {
            let request = common::read_bound(self.backend, &cache["request"], "源缓存重启请求")?;
            if request["previous"] != *original_redis {
                return Err("缓存恢复证明未绑定原 Redis 代次".into());
            }
            if let Some(tools) = cache.get("tools").filter(|value| !value.is_null()) {
                common::bound_file(self.backend, tools, "缓存已验证工具后继")?;
            }
            Some(&cache["redis"])
        };
        let retained_redis = handoff
            .as_ref()
            .map(RustfsHandoff::retained_redis)
            .transpose()?;
        storage["redis"] = select_redis(original_redis, cache_redis, retained_redis)?;
        let review = review(self)?;
        let observer = SystemObserver;
        let inputs = self.inputs();
        rustfs::verify(
            &inputs,
            &review,
            &storage["rustfs"],
            runtime.as_ref(),
            &observer,
        )?;
        redis_layout::verify(&inputs, &review, &storage["redis"], &observer)?;
        kernel::require_observed_identity(&storage["rustfs"]["identity"], &observer)?;
        if common::binding(&run.join("state.json"))? != state
            || registered::registered(self.backend, run, CloneSide::Target, None)? != runtime
            || cache::binding::load(self.backend, run, None)? != cache
        {
            return Err("来源存储观察期间完整账本或已登记代次变化".into());
        }
        if let Some(handoff) = handoff {
            handoff.check(self.backend)?;
        }
        (self.checkpoint)()?;
        Ok(storage)
    }

    pub(crate) fn redis_state(&self, redis: &Value) -> Result<Value> {
        (self.checkpoint)()?;
        let result = if super::warm_redis::applies(self.generation) {
            super::warm_redis::capture(&self.inputs(), redis)?
        } else {
            crate::recovery::clone::native::target_verify::redis::state_with_observer(
                &self.inputs(),
                redis,
                true,
                true,
                &SystemObserver,
            )?
        };
        (self.checkpoint)()?;
        Ok(result)
    }
}

fn select_redis(
    original: &Value,
    cache: Option<&Value>,
    retained: Option<&Value>,
) -> Result<Value> {
    if let (Some(cache), Some(retained)) = (cache, retained)
        && cache != retained
    {
        return Err("复制缓存重启与 RustFS 移交保留的 Redis 代次不一致".into());
    }
    let Some(selected) = cache.or(retained) else {
        return Ok(original.clone());
    };
    let mut allowed = original.clone();
    for field in ["pid", "started", "run_id"] {
        allowed[field] = selected[field].clone();
    }
    if *selected != allowed {
        return Err("Redis 后继改变了原配置、产物、平台、端点或工具".into());
    }
    Ok(allowed)
}

fn review<T>(resources: &Resources<'_, T>) -> Result<Value> {
    let mut binding = document::object(&resources.request["review"], "源 seed 审阅绑定")?.clone();
    let canonical = binding
        .remove("canonical_sha256")
        .ok_or("源审阅缺少规范摘要")?;
    let value = common::read_bound(resources.backend, &Value::Object(binding), "源 seed 审阅")?;
    if canonical != digest::plan_hash(&value)? {
        return Err("源 seed 审阅规范摘要不同".into());
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::select_redis;

    #[test]
    fn source_storage_selects_retained_redis_over_the_initial_process() {
        let initial = json!({
            "configuration": {"path":"redis.conf", "sha256":"original-config"},
            "executable":"redis-server.exe", "platform":"windows", "port":6380,
            "sha256":"original-executable", "tool":{"path":"redis-server.exe"},
            "pid":3, "started":"1000", "run_id":"initial"
        });
        let mut retained = initial.clone();
        retained["pid"] = json!(4);
        retained["started"] = json!("2000");
        retained["run_id"] = json!("retained");
        assert_eq!(select_redis(&initial, None, None).unwrap(), initial);
        assert_eq!(
            select_redis(&initial, None, Some(&retained)).unwrap(),
            retained
        );
        assert_eq!(
            select_redis(&initial, Some(&retained), None).unwrap(),
            retained
        );
        assert_eq!(
            select_redis(&initial, Some(&retained), Some(&retained)).unwrap(),
            retained
        );
        assert!(select_redis(&initial, Some(&initial), Some(&retained)).is_err());
        for field in [
            "configuration",
            "executable",
            "platform",
            "port",
            "sha256",
            "tool",
        ] {
            let mut changed = retained.clone();
            changed[field] = json!("changed");
            assert!(
                select_redis(&initial, None, Some(&changed)).is_err(),
                "{field}"
            );
        }
    }
}

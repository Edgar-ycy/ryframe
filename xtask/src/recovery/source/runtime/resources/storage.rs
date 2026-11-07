//! 从原复制账本取得存储代次，再观察实际二进制、内核、argv、监听与 Redis 配置。
use super::{Resources, Transport, document, json};
use crate::{
    Result,
    cli::CloneSide,
    recovery::clone::native::{
        cache, digest,
        export::transport::{CommandOutput, CommandSpec},
        resources::{common, storage::registered},
        s3,
        target_verify::storage::{SystemObserver, kernel, redis_layout, rustfs},
    },
};
use serde_json::Value;
use std::path::Path;

impl<T: Transport> Resources<'_, T> {
    pub(crate) fn storage_identity(&self, run: &Path) -> Result<Value> {
        (self.checkpoint)()?;
        let state = common::binding(&run.join("state.json"))?;
        let manifest = document::read(&run.join("manifest.json"))?;
        if manifest["copy_stage"] != "source_to_seed" {
            return Err("来源存储观察只消费原 source_to_seed 复制账本".into());
        }
        let runtime = registered::registered(self.backend, run, CloneSide::Target, None)?;
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
        if !cache.is_null() {
            let request = common::read_bound(self.backend, &cache["request"], "源缓存重启请求")?;
            let original = &self.request["storage"]["redis"];
            if request["previous"] != *original {
                return Err("缓存恢复证明未绑定原 Redis 代次".into());
            }
            let mut allowed = original.clone();
            for field in ["pid", "started", "run_id"] {
                allowed[field] = cache["redis"][field].clone();
            }
            if let Some(tools) = cache.get("tools").filter(|value| !value.is_null()) {
                common::bound_file(self.backend, tools, "缓存已验证工具后继")?;
                if original["platform"] != "windows" {
                    allowed["wsl"] = cache["redis"]["wsl"].clone();
                }
            }
            if cache["redis"] != allowed {
                return Err("缓存恢复改变了原配置、产物、端点或未授权工具".into());
            }
            storage["redis"] = allowed;
        }
        let review = review(self)?;
        let transport = KernelLog(self);
        let observer = SystemObserver::new(&transport);
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
        (self.checkpoint)()?;
        Ok(storage)
    }

    pub(crate) fn redis_state(&self, redis: &Value) -> Result<Value> {
        (self.checkpoint)()?;
        let result = crate::recovery::clone::native::target_verify::redis::state_with_observer(
            &self.inputs(),
            redis,
            true,
            true,
            &SystemObserver::new(self.transport),
        )?;
        (self.checkpoint)()?;
        Ok(result)
    }
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

struct KernelLog<'a, T>(&'a Resources<'a, T>);

impl<T: Transport> Transport for KernelLog<'_, T> {
    fn run(&self, spec: CommandSpec) -> Result<CommandOutput> {
        if spec.label != "target-redis-kernel" || spec.stdin.is_some() {
            return Err("来源内核观察拒绝未知命令种类或 stdin".into());
        }
        (self.0.checkpoint)()?;
        let mut command = vec![spec.program.to_string_lossy().into_owned()];
        command.extend(spec.args.clone());
        let directory = spec.work_dir.clone();
        let result = self.0.transport.run(spec);
        let stdout = result
            .as_ref()
            .map(|value| value.stdout.as_slice())
            .unwrap_or_default();
        let stderr = result
            .as_ref()
            .map(|value| value.stderr.as_slice())
            .unwrap_or_default();
        document::write_new(
            &directory.join(format!(
                "redis-kernel-{}.command.json",
                uuid::Uuid::new_v4().simple()
            )),
            &json!({"command":command,"returncode":result.as_ref().ok().and_then(|value|value.status.code()),
                "error_type":if result.is_err(){json!("TransportError")}else{Value::Null},
                "stdout":self.0.redact(&String::from_utf8_lossy(stdout))?,"stderr":self.0.redact(&String::from_utf8_lossy(stderr))?}),
        )?;
        (self.0.checkpoint)()?;
        result
    }
    fn s3(&self, request: s3::Request) -> Result<Value> {
        self.0.transport.s3(request)
    }
}

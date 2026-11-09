//! 复核原 START 的重启现场并投影正式只读命令的结果。
use std::path::Path;

use serde_json::{Value, json};

use super::{build, host, observation};
use crate::{
    Result,
    cli::{SeedSourceOperation, SeedSourceOptions},
    recovery::{
        clone::native::{
            cache, digest, json as document, lifecycle::state, resources::common,
            target_verify::input::request_configuration::canonical_path,
        },
        monitoring::execution::ProtectedInputs,
        runtime::dataset_authority::reads::Window,
    },
};

pub(super) struct Sample {
    pub(super) raw: Value,
    pub(super) value: Value,
    pub(super) reboot_blockers: Vec<String>,
}

pub(in crate::recovery) fn run(backend: &Path, options: &SeedSourceOptions) -> Result<Value> {
    if options.operation != SeedSourceOperation::GenerationRebootStatus
        || options.write
        || options.request.is_some()
    {
        return Err("source-generation-reboot-status 只读且不接受替代请求".into());
    }
    let run = canonical_path(&common::local_directory(
        backend,
        &options.run_dir,
        "源代次重启观察运行目录",
    )?)?;
    let window = Window::direct()?;
    window.finish(sample(backend, &run).map(|observed| observed.value))
}

pub(super) fn sample(backend: &Path, run: &Path) -> Result<Sample> {
    let inputs = ProtectedInputs::open([run.join("state.json"), run.join("manifest.json")])?;
    inputs.finish((|| {
        let ledger = state::load(run, true)?;
        let raw = observation::request(&json!({"backend":backend,"directory":run}))?;
        let start = &raw["start"];
        let number = start["number"]
            .as_u64()
            .ok_or("重启观察缺少源 START 序号")?;
        let boot = raw["proof"]["boot_ticks"]
            .as_u64()
            .ok_or("重启观察缺少主机启动刻度")?;
        let mut reboot_blockers = serde_json::from_value::<Vec<String>>(raw["blockers"].clone())?;
        let (cache, cache_blockers) =
            cache::reboot::observe(backend, run, boot).unwrap_or_else(|_| {
                (
                    json!({"status":"error","error_type":"ValueError"}),
                    vec!["缓存代次无法完成只读观察".to_owned()],
                )
            });
        reboot_blockers.extend(cache_blockers);
        let mut blockers = reboot_blockers.clone();
        let build = build::observe(backend, &raw["facts"])?;
        if build["backend"]["matches"] != true || build["maintenance"]["matches"] != true {
            blockers.push("构建环境与登记收据不一致，收口前必须成对重新登记构建收据".to_owned());
        }
        let history = state::attempts(&ledger)?
            .iter()
            .filter(|row| {
                row["number"]
                    .as_u64()
                    .is_some_and(|attempt| attempt < number)
            })
            .collect::<Vec<_>>();
        let later = document::array(&raw["later"], "重启观察后继阶段")?
            .iter()
            .map(|row| &row["number"])
            .collect::<Vec<_>>();
        let value = json!({
            "status":"source_generation_reboot_observed","attempt":number,"start":start["result"],
            "history_length":history.len(),"history_sha256":digest::plan_hash(&json!(history))?,
            "service_attempts":later,"verification":raw["prefix"],"reboot":raw["proof"],
            "roles":raw["roles"],"storage":raw["storage"],"cache":cache,"build_context":build,
            "blockers":blockers,"closure_ready":blockers.is_empty(),"remote_writes":0,
            "restore_qualified":false,
        });
        if state::load(run, true)? != ledger {
            return Err("重启观察期间完整账本发生变化；保留现场，禁止重放".into());
        }
        host::require_fresh(&raw["proof"])?;
        Ok(Sample {
            raw,
            value,
            reboot_blockers,
        })
    })())
}

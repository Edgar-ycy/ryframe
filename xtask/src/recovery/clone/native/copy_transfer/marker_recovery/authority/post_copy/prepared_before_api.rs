//! 持有成功的 API-only 准备与启动前拒绝证据，直到 Redis 接续收尾。
use std::{collections::BTreeSet, ffi::OsString, fs};

use super::*;
use crate::recovery::clone::native::{cache::handoff::registration, lifecycle::identity};

const CONTROL_GUARD: &str = "runtime-control.guard";
const RUNTIME_FILES: &[&str] = &[
    "binaries.json",
    "handoff.json",
    "runtime.json",
    CONTROL_GUARD,
];
const CONFIG_FILES: &[&str] = &["app.toml", "app.test.toml"];

pub(super) struct Evidence {
    run: PathBuf,
    attempt: PathBuf,
    runtime: PathBuf,
    failed_result: PathBuf,
    api_url: String,
    worker_url: String,
    guard: ProcessGuard,
    directories: Vec<Inputs>,
    files: Inputs,
    artifacts: Inputs,
    pub(super) binding: Value,
}

pub(super) fn rejected_start(row: &Value) -> bool {
    row["stage"] == "runtime-target"
        && row["mode"] == "start"
        && row["status"] == "failed"
        && row["error_type"] == "ApiHandoffPreStartRejected"
        && row["result"].is_null()
        && row["finished_at"].is_string()
}

impl Evidence {
    pub(super) fn load(
        request: &StageRequest<'_>,
        manifest: &Value,
        post: &seed::history::Post,
        rows: PreparedRows<'_>,
    ) -> Result<Self> {
        let backend = request.backend;
        let run = request.run_dir;
        let runtime = run.join("post-copy/runtime");
        if post.runtime != runtime {
            return Err("接续只能绑定原未启动的 API 运行目录".into());
        }
        let guard = ProcessGuard::acquire_existing_named(&runtime, CONTROL_GUARD)?;
        let config = runtime.with_file_name("runtime-configuration");
        let [api, worker] = ["config", "worker-config"].map(|role| config.join(role));
        let prepared = number(rows.prepare)?;
        let rejected = number(rows.failure)?;
        let attempt = run.join(format!("post-copy/attempt-{rejected:04}"));
        let directories = [&runtime, &config, &api, &worker, &attempt]
            .into_iter()
            .map(|path| {
                Inputs::directory(&common::local_directory(backend, path, "原 API 准备目录")?)
            })
            .collect::<Result<Vec<_>>>()?;
        let configured = [
            api.join("app.toml"),
            api.join("app.test.toml"),
            worker.join("app.toml"),
            worker.join("app.test.toml"),
        ];
        let prepared_result =
            common::bound_file(backend, &rows.prepare["result"], "原准备成功结果")?;
        let failed_result = state::result_path(run, rejected);
        let failure = run.join(format!("failure-{rejected:04}.json"));
        let [handoff, contract, binaries] =
            ["handoff.json", "runtime.json", "binaries.json"].map(|name| runtime.join(name));
        let files = hold(
            backend,
            &[
                prepared_result,
                state::controller_path(run, prepared),
                state::controller_path(run, rejected),
                failure.clone(),
                handoff.clone(),
                contract.clone(),
                binaries.clone(),
            ]
            .into_iter()
            .chain(configured.iter().cloned())
            .collect::<Vec<_>>(),
        )?;
        let prepare_controller = closed_controller(run, rows.prepare)?;
        let start_controller = closed_controller(run, rows.failure)?;
        let failure_binding = common::binding(&failure)?;
        verify_failure(
            &common::read_bound(backend, &failure_binding, "原 API 启动前拒绝")?,
            rejected,
            &start_controller,
        )?;
        let frozen = document::read(&contract)?;
        let artifacts = hold_artifacts(backend, &frozen)?;
        if registration::cleanup_runtime(backend, run, post)? != frozen {
            return Err("接续期间原 API 运行合同变化".into());
        }
        let initial = common::read_bound(backend, &manifest["initialized"], "原目标初始化")?;
        let (api_url, worker_url) = verify_runtime(
            &initial["generation"]["selected"],
            &frozen,
            &document::read(&handoff)?,
            &api,
            &worker,
        )?;
        let evidence = Self {
            run: run.to_owned(),
            attempt,
            runtime,
            failed_result,
            api_url,
            worker_url,
            guard,
            directories,
            files,
            artifacts,
            binding: json!({
                "prepare": rows.prepare["result"],
                "prepare_controller": prepare_controller,
                "handoff": common::binding(&handoff)?,
                "runtime": common::binding(&contract)?,
                "binaries": common::binding(&binaries)?,
                "configuration": configured.iter().map(|path| common::binding(path)).collect::<Result<Vec<_>>>()?,
                "failure": failure_binding,
                "start_controller": start_controller,
            }),
        };
        evidence.check()?;
        Ok(evidence)
    }

    pub(super) fn check(&self) -> Result<()> {
        self.guard.check_named(&self.runtime, CONTROL_GUARD)?;
        for directory in &self.directories {
            directory.check()?;
        }
        self.files.check()?;
        #[cfg(windows)]
        self.artifacts.check_identities()?;
        #[cfg(not(windows))]
        self.artifacts.check()?;
        absent(&self.failed_result)?;
        for name in [
            "post-copy/handoff.json",
            "seed-runtime.json",
            "seed-runtime",
        ] {
            absent(&self.run.join(name))?;
        }
        exact_members(&self.runtime, RUNTIME_FILES)?;
        let config = self.runtime.with_file_name("runtime-configuration");
        exact_members(&config, &["config", "worker-config"])?;
        exact_members(&config.join("config"), CONFIG_FILES)?;
        exact_members(&config.join("worker-config"), CONFIG_FILES)?;
        exact_members(&self.attempt, &[])?;
        common::require_closed_runtime_ports(&self.api_url, &self.worker_url)?;
        self.guard.check_named(&self.runtime, CONTROL_GUARD)
    }
}

fn hold_artifacts(backend: &Path, contract: &Value) -> Result<Inputs> {
    let paths = ["api", "worker"]
        .into_iter()
        .map(|role| {
            let path = document::text(&contract["artifacts"][role]["path"], "原准备产品产物")?;
            protected_file(backend, Path::new(path))
        })
        .collect::<Result<Vec<_>>>()?;
    Inputs::open(paths)
}

fn verify_runtime(
    selected: &Value,
    frozen: &Value,
    handoff: &Value,
    api: &Path,
    worker: &Path,
) -> Result<(String, String)> {
    let api_url = document::text(&selected["api_url"], "原 API 地址")?;
    let worker_url = document::text(&selected["worker_ready_url"], "原 Worker 就绪地址")?;
    if handoff["api_url"] != api_url
        || frozen["worker_ready_url"] != worker_url
        || frozen["scope_id"] != selected["scope_id"]
        || frozen["api_config_dir"] != json!(api)
        || frozen["worker_config_dir"] != json!(worker)
    {
        return Err("接续必须保留原 API、Worker、作用域及配置路径".into());
    }
    Ok((api_url.to_owned(), worker_url.to_owned()))
}

fn number(row: &Value) -> Result<u64> {
    row["number"]
        .as_u64()
        .ok_or_else(|| "原准备或启动阶段缺少序号".into())
}

fn closed_controller(run: &Path, row: &Value) -> Result<Value> {
    let (binding, owner) =
        state::controller_record(run, number(row)?, &failed_prepare::running(row))?;
    identity::validate(&owner, run)?;
    if owner["manifest_sha256"] != state::manifest_binding(run)?["sha256"] {
        return Err("原准备或启动控制器属于其他运行清单".into());
    }
    let identity = &owner["identity"];
    let pid = u32::try_from(identity["pid"].as_u64().ok_or("原控制器 PID 无效")?)?;
    if common::process_identity(pid)?.as_ref() == Some(identity) {
        return Err("原准备或启动控制器仍在运行".into());
    }
    Ok(binding)
}

fn verify_failure(value: &Value, number: u64, controller: &Value) -> Result<()> {
    if *value
        != json!({
            "format_version":2,"kind":"devex-stage-failure","attempt":number,
            "stage":"runtime-target","mode":"start","error_type":"ApiHandoffPreStartRejected",
            "operation_id":"clone.runtime-target.start","controller":controller
        })
    {
        return Err("原启动前失败收据未精确绑定启动操作与控制器".into());
    }
    Ok(())
}

fn exact_members(directory: &Path, expected: &[&str]) -> Result<()> {
    let actual = fs::read_dir(directory)?
        .map(|item| item.map(|entry| entry.file_name()))
        .collect::<std::io::Result<BTreeSet<_>>>()?;
    let expected = expected
        .iter()
        .copied()
        .map(OsString::from)
        .collect::<BTreeSet<_>>();
    if actual != expected {
        return Err("原准备窗口出现启动意图、生产者或未知文件".into());
    }
    Ok(())
}

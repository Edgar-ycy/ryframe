//! 原源代次重启闭合只发布已观察的本地证据。
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::{CLOSE, Sample};
use crate::{
    Result,
    cli::{SeedSourceOperation, SeedSourceOptions},
    local_test_path::same_local_test_file,
    recovery::{
        clone::native::{
            json as document,
            lifecycle::{
                identity, lock,
                stage::{self, StageRequest, StageRunner},
                state,
            },
            resources::common,
            target_verify::{input::request_configuration::canonical_path, publication},
        },
        monitoring::execution::ProtectedInputs,
        runtime::dataset_authority::reads::Window,
    },
};

const STAGE: &str = "seed-runtime";
const START_ATTEMPT: u64 = 70;
const CLOSE_ATTEMPT: u64 = START_ATTEMPT + 1;

pub(in crate::recovery) fn run(backend: &Path, options: &SeedSourceOptions) -> Result<Value> {
    if options.operation != SeedSourceOperation::GenerationRebootClose || !options.write {
        return Err("源代次重启闭合必须经正式入口并显式授权 --write".into());
    }
    let selected = options
        .request
        .as_deref()
        .ok_or("重启闭合必须明确 --request")?;
    let run = canonical_path(&common::local_directory(
        backend,
        &options.run_dir,
        "源代次重启闭合运行目录",
    )?)?;
    let result = stage::run_named(backend, &run, STAGE, CLOSE, &Runner { selected })?;
    Ok(json!({
        "status":result["status"],"stage":result["stage"],"mode":result["mode"],
        "attempt":result["attempt"],"restore_qualified":result["restore_qualified"],
    }))
}

struct Runner<'a> {
    selected: &'a Path,
}

impl StageRunner for Runner<'_> {
    fn preflight(&self, request: &StageRequest<'_>) -> Result<()> {
        if request.attempt.is_some() {
            return Err("重启闭合预检不能使用已登记 attempt".into());
        }
        self.with_bound(request, |bound, _| bound.observe().map(|_| ()))
    }

    fn execute(&self, request: &StageRequest<'_>) -> Result<Value> {
        if request.attempt != Some(CLOSE_ATTEMPT) {
            return Err("重启闭合必须绑定 C70 后唯一相邻 attempt".into());
        }
        self.with_bound(request, |bound, window| {
            let observed = bound.observe()?;
            bound.publish(&observed, window)
        })
    }
}

impl Runner<'_> {
    fn with_bound<T>(
        &self,
        request: &StageRequest<'_>,
        operation: impl FnOnce(&Bound<'_>, &Window) -> Result<T>,
    ) -> Result<T> {
        let window = Window::direct()?;
        let result = (|| {
            let bound = Bound::capture(request, self.selected)?;
            let result = operation(&bound, &window);
            finish(result, bound.check())
        })();
        window.finish(result)
    }
}

struct Bound<'a> {
    backend: &'a Path,
    run: &'a Path,
    selected: PathBuf,
    protected: ProtectedInputs,
    manifest: Value,
    ledger: Value,
    state: Value,
    start: Value,
    active: Option<Active>,
    generation: PathBuf,
    generation_id: (u64, u64),
}

impl<'a> Bound<'a> {
    fn capture(request: &StageRequest<'a>, selected: &Path) -> Result<Self> {
        if request.stage != STAGE || request.mode != CLOSE {
            return Err("重启闭合只允许正式 seed-runtime 阶段".into());
        }
        let backend = request.backend;
        let run = request.run_dir;
        let selected = common::common_file(backend, selected, "C70 显式原请求")?;
        let protected = ProtectedInputs::open([selected.clone()])?;
        let manifest = common::binding(&run.join("manifest.json"))?;
        let state = common::binding(&run.join("state.json"))?;
        let value = common::read_bound(backend, &manifest, "重启闭合原清单")?;
        if value["copy_stage"] != "source_to_seed" {
            return Err("重启闭合只允许 source_to_seed 原运行".into());
        }
        let ledger = state::load(run, true)?;
        let attempts = state::attempts(&ledger)?;
        let start = start_before_close(attempts, request.attempt)?.clone();
        let active = match request.attempt {
            Some(number) => Some(Active::read(run, &manifest, &ledger, number)?),
            None => None,
        };
        let generation = run.join("g0070");
        let generation_id = directory_identity(backend, &generation)?;
        let bound = Self {
            backend,
            run,
            selected,
            protected,
            manifest,
            ledger,
            state,
            start,
            active,
            generation,
            generation_id,
        };
        bound.check()?;
        Ok(bound)
    }

    fn check(&self) -> Result<()> {
        self.protected.assert_unchanged()?;
        common::bound_file(self.backend, &self.manifest, "重启闭合原清单")?;
        common::bound_file(self.backend, &self.state, "重启闭合原账本")?;
        if state::load(self.run, true)? != self.ledger
            || directory_identity(self.backend, &self.generation)? != self.generation_id
        {
            return Err("重启闭合原阶段账本或 C70 目录身份变化；保留现场".into());
        }
        if let Some(active) = &self.active
            && Active::read(self.run, &self.manifest, &self.ledger, CLOSE_ATTEMPT)? != *active
        {
            return Err("重启闭合原持锁控制器变化；保留现场".into());
        }
        self.protected.assert_unchanged()
    }

    fn require_new_output(&self) -> Result<PathBuf> {
        let output = self.generation.join("reboot-close");
        require_absent(&output)?;
        require_absent(&state::result_path(self.run, CLOSE_ATTEMPT))?;
        Ok(output)
    }

    fn observe(&self) -> Result<Sample> {
        self.check()?;
        self.require_new_output()?;
        let observed = super::sample(self.backend, self.run)?;
        if !observed.reboot_blockers.is_empty() {
            return Err(format!(
                "主机重启事实尚未完整核实：{}",
                observed.reboot_blockers.join("；")
            )
            .into());
        }
        if !document::array(&observed.value["service_attempts"], "重启后服务阶段")?.is_empty()
            || observed.raw["start"] != self.start
            || Path::new(document::text(
                &observed.raw["facts"]["output"],
                "C70 目录",
            )?) != self.generation
        {
            return Err("重启闭合必须属于同一原 START，且早于新服务代次登记".into());
        }
        let original = &observed.raw["facts"]["receipt"]["request"];
        let recorded = common::bound_file(self.backend, original, "C70 冻结原请求")?;
        if !same_local_test_file(self.backend, &self.selected, &recorded)? {
            return Err("重启闭合必须绑定 C70 冻结的同一正式请求文件".into());
        }
        self.check()?;
        self.require_new_output()?;
        Ok(observed)
    }

    fn publish(&self, observed: &Sample, window: &Window) -> Result<Value> {
        let mut receipt = closure(&observed.raw, &observed.value["cache"]);
        receipt.sort_all_objects();
        window.finish(self.check())?;
        let output = self.require_new_output()?;
        common::ensure_new_directory(self.backend, &output, "C70 重启闭合目录")?;
        let output_id = directory_identity(self.backend, &output)?;
        self.check()?;
        let path = output.join("closure.json");
        publication::publish(&path, &receipt)?;
        let binding = common::binding(&path)?;
        if common::read_bound(self.backend, &binding, "已发布重启闭合收据")? != receipt
            || directory_identity(self.backend, &output)? != output_id
        {
            return Err("重启闭合发布后原内容或目录身份变化；保留现场，禁止重放".into());
        }
        self.check()?;
        Ok(outer_result(&receipt, binding))
    }
}

#[derive(PartialEq, Eq)]
struct Active {
    controller: Value,
    lock: (Value, Value),
    lock_id: (u64, u64),
}

impl Active {
    fn read(run: &Path, manifest: &Value, ledger: &Value, number: u64) -> Result<Self> {
        let attempt = state::running_attempt(ledger)?.ok_or("重启闭合缺少当前唯一运行阶段")?;
        let (controller, owner) = state::controller_record(run, number, attempt)?;
        let locked = lock::owner_observation(run)?.ok_or("重启闭合缺少原 run.lock")?;
        let lock_path = run.join("run.lock");
        let lock_id = common::device_inode_for(&lock_path, &fs::symlink_metadata(&lock_path)?)?;
        if owner != locked.1
            || owner["manifest_sha256"] != manifest["sha256"]
            || owner["identity"] != identity::current()?
            || lock::owner_observation(run)? != Some(locked.clone())
        {
            return Err("重启闭合要求同一原清单及当前实际创建身份持有运行锁".into());
        }
        Ok(Self {
            controller,
            lock: locked,
            lock_id,
        })
    }
}

fn start_before_close(attempts: &[Value], active: Option<u64>) -> Result<&Value> {
    let prefix = if let Some(number) = active {
        let (last, prefix) = attempts.split_last().ok_or("重启闭合缺少账本尾段")?;
        if number != CLOSE_ATTEMPT
            || last["number"].as_u64() != Some(number)
            || last["stage"] != STAGE
            || last["mode"] != CLOSE
            || last["status"] != "running"
            || !last["result"].is_null()
            || !last["finished_at"].is_null()
            || !last["error_type"].is_null()
        {
            return Err("重启闭合不属于当前唯一末尾运行阶段".into());
        }
        prefix
    } else {
        attempts
    };
    if prefix
        .iter()
        .any(|row| row["status"] == "running" || row["stage"] == STAGE && row["mode"] == CLOSE)
    {
        return Err("存在活动阶段或已登记的重启闭合；保留原账本，禁止重放".into());
    }
    let last = prefix.last().ok_or("重启闭合缺少相邻的原 C70 START")?;
    if last["number"].as_u64() != Some(START_ATTEMPT)
        || last["stage"] != STAGE
        || last["mode"] != "source-generation-start"
        || last["status"] != "passed"
        || !last["result"].is_object()
        || !last["error_type"].is_null()
    {
        return Err("重启闭合必须紧随原已成功的 C70 START".into());
    }
    Ok(last)
}

fn require_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err("重启闭合已有目录或阶段结果；保留现场，禁止覆盖或重放".into()),
    }
}

fn directory_identity(backend: &Path, path: &Path) -> Result<(u64, u64)> {
    common::local_directory(backend, path, "重启闭合证据目录")?;
    common::device_inode_for(path, &fs::symlink_metadata(path)?)
}

fn closure(raw: &Value, cache: &Value) -> Value {
    let start = &raw["facts"]["receipt"];
    json!({
        "format_version":1,"kind":"seed-source-generation-reboot-closure",
        "status":"seed_source_generation_reboot_closed","start":raw["start"]["result"],
        "request":start["request"],"failed_prefix":raw["prefix"],"reboot":raw["proof"],
        "roles":raw["roles"],"storage":raw["storage"],"cache":cache,
        "baseline":start["before"],"running":start["running"],
        "declared_deltas":{"sys_login_info":11,"sys_oper_log":0,"sys_outbox_event":0},
        "verification_qualified":false,"replay_allowed":false,"remote_writes":0,"restore_qualified":false,
    })
}

fn outer_result(receipt: &Value, binding: Value) -> Value {
    json!({
        "status":receipt["status"],"start":receipt["start"],"request":receipt["request"],
        "closure":binding,"baseline":receipt["baseline"],"reboot":receipt["reboot"],
        "failed_prefix":receipt["failed_prefix"],"declared_deltas":receipt["declared_deltas"],
        "producer":receipt["failed_prefix"]["producer"],"verification_qualified":false,
        "replay_allowed":false,"remote_writes":0,"restore_qualified":false,
    })
}

fn finish<T>(result: Result<T>, after: Result<()>) -> Result<T> {
    match (result, after) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(after)) => Err(format!("{error}；重启闭合后验失败：{after}").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_requires_immediate_c70_and_rejects_consumed_close_attempt() {
        let start = json!({"number":70,"stage":STAGE,"mode":"source-generation-start",
            "status":"passed","result":{"fixture":"start"},"error_type":null});
        let running = json!({"number":71,"stage":STAGE,"mode":CLOSE,"status":"running",
            "result":null,"error_type":null,"finished_at":null});
        assert_eq!(
            start_before_close(std::slice::from_ref(&start), None).unwrap(),
            &start
        );
        assert!(start_before_close(&[start.clone(), running.clone()], None).is_err());
        assert_eq!(
            start_before_close(&[start.clone(), running.clone()], Some(71)).unwrap(),
            &start
        );
        let mut failed = running.clone();
        failed["status"] = json!("failed");
        failed["error_type"] = json!("ControllerInterrupted");
        assert!(start_before_close(&[start.clone(), failed.clone()], None).is_err());
        let mut late = running;
        late["number"] = json!(72);
        assert!(start_before_close(&[start, failed, late], Some(72)).is_err());
    }

    #[test]
    fn close_retains_exact_receipt_and_outer_producer_fields() {
        let raw = json!({"start":{"result":"start"},"facts":{"receipt":{"request":"request",
            "before":"baseline","running":"running"}},"prefix":{"producer":{"completion":"original"}},
            "proof":"host","roles":"roles","storage":"storage"});
        let receipt = closure(&raw, &json!("cache"));
        assert_eq!(receipt.as_object().unwrap().len(), 17);
        assert_eq!(receipt["cache"], "cache");
        assert_eq!(receipt["running"], "running");
        assert_eq!(
            receipt["declared_deltas"],
            json!({"sys_login_info":11,"sys_oper_log":0,"sys_outbox_event":0})
        );
        let outer = outer_result(&receipt, json!("closure"));
        assert_eq!(outer.as_object().unwrap().len(), 13);
        assert_eq!(outer["producer"], raw["prefix"]["producer"]);
        assert_eq!(outer["baseline"], "baseline");
        assert_eq!(outer["replay_allowed"], false);
        assert_eq!(outer["remote_writes"], 0);
    }
}

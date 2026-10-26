//! 一次缓存尝试与独立执行结果；已有尝试永不再次授权 EXEC。
use super::super::super::cache;
use super::{cache_context, documents, receipts};
use crate::{
    Result,
    recovery::clone::native::{digest, resources::common},
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(super) const ATTEMPT: &str = "cache-cleanup-attempt.json";
pub(super) const EXECUTION: &str = "cache-cleanup-execution.json";

pub(super) struct Frame {
    backend: PathBuf,
    directory: PathBuf,
    pub(super) request: Value,
    bindings: Value,
}

impl Frame {
    pub(super) fn open(args: &Value) -> Result<Self> {
        let (backend, directory) = receipts::directory(args)?;
        if Path::new(documents::text(&args["output"], "缓存清理输出")?)
            != directory.join("cache-cleanup.json")
        {
            return Err("缓存清理输出必须属于同一 verification；保留现场核对".into());
        }
        let (request, bindings) = cache_context::load(&backend, &directory)?;
        Ok(Self {
            backend,
            directory,
            request,
            bindings,
        })
    }
    fn unchanged(&self) -> Result<()> {
        if cache_context::load(&self.backend, &self.directory)?.1 != self.bindings {
            return Err("缓存尝试的源、请求、生产者或登录审计绑定已变化".into());
        }
        Ok(())
    }
    pub(super) fn create(&self, observation: &Value) -> Result<Value> {
        if self.state()?["status"] != "not_attempted"
            || self.directory.join("cache-cleanup.json").try_exists()?
        {
            return Err("同代缓存已有尝试或清理收据，禁止重试；核对原结果".into());
        }
        validate_observation(observation, &self.bindings)?;
        let recovery_path = self
            .directory
            .join("source-verification-recovery-intent.json");
        let recovery = if recovery_path.try_exists()? {
            Some(common::binding(&recovery_path)?)
        } else {
            None
        };
        if self.directory.join("failed.json").try_exists()? != recovery.is_some() {
            return Err("失败来源只能在完整已登记 recovery intent 下首次尝试缓存清理".into());
        }
        if let Some(binding) = &recovery {
            validate_recovery(&self.backend, binding, observation, &self.bindings)?;
            super::recovery_create::prefix(
                &json!({"backend":self.backend,"directory":self.directory}),
            )?;
            let intent = common::read_bound(&self.backend, binding, "缓存首次恢复意图")?;
            super::recovery::pending(&json!({"backend":self.backend,"directory":self.directory,
                "start":self.bindings["source_generation"],"lineage":self.bindings["dataset_lineage"],
                "before":common::binding(&self.directory.join("before/image.json"))?,
                "successor":intent["successor"],"subjects":self.bindings["subjects"]}))?;
        }
        let value = json!({"format_version":1,"kind":"source-cache-cleanup-attempt","recovery_intent":recovery,
            "bindings":self.bindings,"scope_id":self.bindings["scope_id"],"observation":observation,
            "observation_sha256":digest::plan_hash(observation)?,"started_at":chrono::Utc::now().to_rfc3339()});
        common::write_new(&self.directory.join(ATTEMPT), &value)?;
        self.unchanged()?;
        common::binding(&self.directory.join(ATTEMPT))
    }
    pub(super) fn finish(&self, attempt: &Value, outcome: &Value) -> Result<Value> {
        self.unchanged()?;
        if common::binding(&self.directory.join(ATTEMPT))? != *attempt {
            return Err("缓存 attempt 已变化".into());
        }
        let status = match outcome["status"].as_str() {
            Some("confirmed") => "confirmed",
            Some("watch_conflict") => "watch_conflict",
            Some("unknown_exec_result") => "unknown_exec",
            Some("precondition_failed" | "pre_exec_io_failed" | "unsupported_watch_expiration") => {
                "pre_exec_failed"
            }
            _ => return Err("缓存执行结果分类未知".into()),
        };
        let value = json!({"format_version":1,"kind":"source-cache-cleanup-execution","attempt":attempt,
            "status":status,"cause":outcome["status"],"deleted":outcome["deleted"],"expired":outcome["expired"],
            "completed_at":chrono::Utc::now().to_rfc3339()});
        common::write_new(&self.directory.join(EXECUTION), &value)?;
        self.state()
    }
    pub(super) fn state(&self) -> Result<Value> {
        self.unchanged()?;
        let a = self.directory.join(ATTEMPT);
        let e = self.directory.join(EXECUTION);
        if !a.try_exists()? {
            if e.try_exists()? {
                return Err("缓存 execution 没有原 attempt；禁止修补或执行".into());
            }
            return Ok(
                json!({"status":"not_attempted","attempt":null,"execution":null,"observation":null,"result":null}),
            );
        }
        let descriptor = common::binding(&a)?;
        let value = common::read_bound(&self.backend, &descriptor, "缓存 attempt")?;
        documents::exact(
            &value,
            &[
                "format_version",
                "kind",
                "bindings",
                "scope_id",
                "observation",
                "observation_sha256",
                "started_at",
                "recovery_intent",
            ],
            "缓存 attempt",
        )?;
        if value["format_version"] != 1
            || value["kind"] != "source-cache-cleanup-attempt"
            || value["bindings"] != self.bindings
            || value["scope_id"] != self.bindings["scope_id"]
            || value["observation_sha256"] != digest::plan_hash(&value["observation"])?
        {
            return Err("缓存 attempt 原绑定或冻结前像已损坏；保留现场".into());
        }
        validate_observation(&value["observation"], &self.bindings)?;
        if !value["recovery_intent"].is_null() {
            validate_recovery(
                &self.backend,
                &value["recovery_intent"],
                &value["observation"],
                &self.bindings,
            )?;
        }
        super::model::time_order(&json!({"values":[value["started_at"]],"label":"缓存尝试"}))?;
        let mut state = json!({"status":"unknown_exec","attempt":descriptor,"execution":null,"observation":value["observation"],"result":null,"attempt_phase":if value["recovery_intent"].is_null(){"normal"}else{"recovery"}});
        if e.try_exists()? {
            let descriptor = common::binding(&e)?;
            let result = common::read_bound(&self.backend, &descriptor, "缓存 execution")?;
            validate_result(&result, &state["attempt"], &value)?;
            state["status"] = result["status"].clone();
            state["execution"] = descriptor;
            state["result"] = result;
        }
        Ok(state)
    }
}

fn validate_observation(value: &Value, bindings: &Value) -> Result<()> {
    cache::observation(&json!({"value":value,"subjects":bindings["subjects"]}))?;
    if value["namespace"]
        != format!(
            "ryframe:{{{}}}:",
            documents::text(&bindings["scope_id"], "缓存 scope")?
        )
    {
        return Err("缓存冻结前像 scope 不属于原请求".into());
    }
    Ok(())
}

fn validate_result(result: &Value, attempt: &Value, original: &Value) -> Result<()> {
    documents::exact(
        result,
        &[
            "format_version",
            "kind",
            "attempt",
            "status",
            "cause",
            "deleted",
            "expired",
            "completed_at",
        ],
        "缓存 execution",
    )?;
    let expired = original["observation"]["before"]
        .as_array()
        .ok_or("缓存前像缺失")?
        .iter()
        .filter(|row| row["snapshot"]["type"] == "none")
        .map(|row| row["snapshot"]["key"].clone())
        .collect::<Vec<_>>();
    let valid = match result["status"].as_str() {
        Some("confirmed") => {
            result["cause"] == "confirmed"
                && result["deleted"].as_u64() == Some(33 - expired.len() as u64)
                && result["expired"] == json!(expired)
        }
        Some("pre_exec_failed") => {
            matches!(
                result["cause"].as_str(),
                Some("precondition_failed" | "pre_exec_io_failed" | "unsupported_watch_expiration")
            ) && result["deleted"].is_null()
                && result["expired"] == json!([])
        }
        Some("watch_conflict") => {
            result["cause"] == "watch_conflict"
                && result["deleted"].is_null()
                && result["expired"] == json!([])
        }
        Some("unknown_exec") => {
            result["cause"] == "unknown_exec_result"
                && result["deleted"].is_null()
                && result["expired"] == json!([])
        }
        _ => false,
    };
    if result["format_version"] != 1
        || result["kind"] != "source-cache-cleanup-execution"
        || result["attempt"] != *attempt
        || !valid
    {
        return Err("缓存执行结果与原 attempt/分类不一致，禁止推断或重放".into());
    }
    super::model::time_order(
        &json!({"values":[original["started_at"],result["completed_at"]],"label":"缓存结果"}),
    )?;
    Ok(())
}

pub(crate) fn state(args: &Value) -> Result<Value> {
    documents::exact(
        args,
        &["backend", "directory", "output"],
        "缓存尝试状态参数",
    )?;
    Frame::open(args)?.state()
}

fn validate_recovery(
    backend: &Path,
    binding: &Value,
    observation: &Value,
    bindings: &Value,
) -> Result<()> {
    let value = common::read_bound(backend, binding, "缓存恢复授权")?;
    let path = common::bound_file(backend, binding, "缓存恢复授权")?;
    let producer_path =
        common::bound_file(backend, &bindings["producer"]["intent"], "原生产者意图")?;
    let directory = producer_path.parent().ok_or("缓存恢复授权缺少目录")?;
    documents::exact(&value, super::recovery::INTENT_FIELDS, "缓存恢复授权")?;
    if path != directory.join("source-verification-recovery-intent.json")
        || value["format_version"] != 1
        || value["kind"] != "restore-source-verification-recovery-intent"
        || value["status"] != "recovering"
        || value["failure"] != common::binding(&directory.join("failed.json"))?
        || value["before"] != common::binding(&directory.join("before/image.json"))?
        || value["dataset_lineage"] != bindings["dataset_lineage"]
    {
        return Err("缓存恢复授权未绑定原失败与完整前像".into());
    }
    super::recovery_create::failure(directory)?;
    if value["source_generation"] != bindings["source_generation"]
        || value["producer"] != bindings["producer"]
        || value["login_audit"] != bindings["login_audit"]
        || value["authorization_cache"] != *observation
    {
        return Err("缓存首次恢复尝试未绑定原来源、生产者、审计与完整冻结前像".into());
    }
    Ok(())
}

pub(super) fn validate_cleanup(backend: &Path, directory: &Path, value: &Value) -> Result<Value> {
    let state = state(
        &json!({"backend":backend,"directory":directory,"output":directory.join("cache-cleanup.json")}),
    )?;
    if value["attempt"] != state["attempt"]
        || value["execution_record"] != state["execution"]
        || value["attempt_phase"] != state["attempt_phase"]
        || value["before"] != state["observation"]["before"]
        || value["namespace"] != state["observation"]["namespace"]
    {
        return Err("缓存清理收据没有绑定原单次 attempt 和完整前像".into());
    }
    match state["status"].as_str() {
        Some("confirmed")
            if value["execution"] == "confirmed_watch_exec"
                && value["deleted"] == state["result"]["deleted"]
                && value["expired_snapshot_keys"] == state["result"]["expired"] => {}
        Some("unknown_exec")
            if value["execution"] == "reconciled_unknown_exec_result"
                && value["deleted"].is_null() => {}
        _ => {
            return Err(
                "明确未执行、冲突或无尝试不能转换为缓存清理成功；核对原分类，不重试".into(),
            );
        }
    }
    Ok(state)
}

pub(crate) fn receipt(args: &Value) -> Result<Value> {
    documents::exact(
        args,
        &["backend", "directory", "value", "subjects"],
        "来源缓存绑定收据参数",
    )?;
    let (backend, directory) = receipts::directory(args)?;
    let value = cache::receipt(&json!({"value":args["value"],"subjects":args["subjects"]}))?;
    validate_cleanup(&backend, &directory, &value)?;
    Ok(value)
}

/// 恢复决策只能消费本地完整绑定结果，不能信任调用方自报执行分类。
pub(crate) fn resume(args: &Value) -> Result<Value> {
    documents::exact(
        args,
        &[
            "backend",
            "directory",
            "output",
            "frozen",
            "types",
            "observed",
        ],
        "缓存恢复决策参数",
    )?;
    let state = Frame::open(args)?.state()?;
    cache::resume(
        &json!({"frozen":args["frozen"],"types":args["types"],"observed":args["observed"],"attempt":state}),
    )
}

/// 目录只接受零个证据、单独 attempt、或绑定该 attempt 的 execution。
pub(super) fn entries(backend: &Path, directory: &Path) -> Result<Vec<&'static str>> {
    let a = directory.join(ATTEMPT).try_exists()?;
    let e = directory.join(EXECUTION).try_exists()?;
    if e && !a {
        return Err("缓存结果缺少原 attempt，保留现场核对".into());
    }
    if !a {
        return Ok(Vec::new());
    }
    state(
        &json!({"backend":backend,"directory":directory,"output":directory.join("cache-cleanup.json")}),
    )?;
    Ok(if e {
        vec![ATTEMPT, EXECUTION]
    } else {
        vec![ATTEMPT]
    })
}

pub(super) fn historical(cleanup: &Value) -> Result<&'static str> {
    match (
        cleanup["attempt_phase"].as_str(),
        cleanup["execution"].as_str(),
    ) {
        (Some("recovery"), Some("confirmed_watch_exec" | "reconciled_unknown_exec_result")) => {
            Ok("not_attempted")
        }
        (Some("normal"), Some("confirmed_watch_exec")) => Ok("confirmed"),
        (Some("normal"), Some("reconciled_unknown_exec_result")) => Ok("unknown"),
        _ => Err("缓存清理原尝试阶段与结果不一致".into()),
    }
}

pub(super) fn confirmed(cleanup: &Value) -> Result<&'static str> {
    historical(cleanup)?;
    Ok(
        if cleanup["execution"] == "reconciled_unknown_exec_result" {
            "reconciled_unknown_exec_result_all_keys_absent"
        } else if cleanup["attempt_phase"] == "normal" {
            "republished_confirmed_watch_exec"
        } else {
            "recovery_watch_exec_unlink"
        },
    )
}

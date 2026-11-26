//! 部门工具修正绑定前次真实失败、原 API 发布及本次独立构建。

use super::{
    Mode, RegisteredPlan, build, common, controller, controller_owner, document, existing, is_plan,
    number_field, old_producer, output, project, stage_attempt, stage_mode, state,
};
use crate::{Result, recovery::clone::native::seed::continuation_tool};
use serde_json::{Value, json};
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

const PURPOSE: &str = "department-continuation";

#[path = "correction/runtime.rs"]
mod runtime;

pub(crate) use runtime::path as runtime_path;

pub(crate) fn path(run: &Path) -> PathBuf {
    run.join("seed-runtime/department-tool-correction.json")
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Generation {
    Bridge,
    Runtime,
}

impl Generation {
    fn path(self, run: &Path) -> PathBuf {
        match self {
            Self::Bridge => path(run),
            Self::Runtime => runtime_path(run),
        }
    }
}

pub(super) fn present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

pub(super) struct Scope<'a> {
    backend: &'a Path,
    run: &'a Path,
    attempts: &'a [Value],
    api: &'a Value,
    original: &'a RegisteredPlan<'a>,
    publication: &'a Value,
}

impl<'a> Scope<'a> {
    pub(super) fn new(
        backend: &'a Path,
        run: &'a Path,
        attempts: &'a [Value],
        api: &'a Value,
        original: &'a RegisteredPlan<'a>,
        publication: &'a Value,
    ) -> Self {
        Self {
            backend,
            run,
            attempts,
            api,
            original,
            publication,
        }
    }

    pub(super) fn select(
        &self,
        number: u64,
        build_request: Option<&Path>,
        generation: Generation,
    ) -> Result<(Value, Value)> {
        let filename = generation.path(self.run);
        if let Some((binding, receipt)) = existing(self.backend, &filename)? {
            let first = number_field(&receipt, "first_attempt")?;
            if build_request.is_some() || first > number {
                return Err("部门修正工具已登记，只能沿用本次阶段的原登记".into());
            }
            self.require_launch_generation(generation, first, number)?;
            self.validate(&receipt, generation)?;
            return Ok((receipt, binding));
        }
        let request = build_request.ok_or("部门修正必须提供经过独立验证的构建请求")?;
        let row = stage_attempt(self.attempts, number)?;
        if !is_plan(row) || row["status"] != "running" {
            return Err("仅当前正式部门规划可以登记修正工具".into());
        }
        let saved = state::load(self.run, false)?;
        if state::attempts(&saved)? != self.attempts {
            return Err("部门修正登记前正式阶段已变化".into());
        }
        require_no_plan(self.run)?;
        require_no_child(self.backend, &output(self.run, number))?;
        let (controller, owner) = controller_owner(self.backend, self.run, row)?;
        let (build_binding, tool) =
            continuation_tool::bind(self.backend, request, PURPOSE, &owner["identity"])?;
        let receipt = self.expected(generation, number, &controller, &build_binding, &tool)?;
        require_no_plan(self.run)?;
        require_no_child(self.backend, &output(self.run, number))?;
        if state::load(self.run, false)? != saved {
            return Err("部门修正登记期间正式阶段变化".into());
        }
        document::publish_new(&filename, &receipt, "部门修正工具")?;
        let binding = common::binding(&filename)?;
        if common::read_bound(self.backend, &binding, "部门修正工具")? != receipt {
            return Err("部门修正工具登记变化，保留原收据".into());
        }
        Ok((receipt, binding))
    }

    pub(super) fn require_launch_generation(
        &self,
        generation: Generation,
        first: u64,
        number: u64,
    ) -> Result<()> {
        if generation == Generation::Bridge
            && first < number
            && is_plan(stage_attempt(self.attempts, number)?)
            && stage_attempt(self.attempts, first)?["status"] == "failed"
        {
            return Err("原部门修正首轮已失败，后续规划需要独立登记的构建接续".into());
        }
        Ok(())
    }

    pub(super) fn validate(&self, receipt: &Value, generation: Generation) -> Result<()> {
        let first = number_field(receipt, "first_attempt")?;
        let row = stage_attempt(self.attempts, first)?;
        let (controller, owner) = controller_owner(self.backend, self.run, row)?;
        let build_binding = &receipt["build_request"];
        let tool =
            continuation_tool::verify(self.backend, build_binding, PURPOSE, &owner["identity"])?;
        if self.expected(generation, first, &controller, build_binding, &tool)? != *receipt {
            return Err("部门修正工具没有绑定原失败、API 发布和独立构建".into());
        }
        Ok(())
    }

    fn expected(
        &self,
        generation: Generation,
        first: u64,
        controller: &Value,
        build_binding: &Value,
        tool: &Value,
    ) -> Result<Value> {
        match generation {
            Generation::Bridge => self.receipt(first, controller, build_binding, tool),
            Generation::Runtime => self.runtime_receipt(first, controller, build_binding, tool),
        }
    }

    fn receipt(
        &self,
        first: u64,
        controller: &Value,
        build_binding: &Value,
        tool: &Value,
    ) -> Result<Value> {
        let api_number = number_field(self.api, "number")?;
        if stage_mode(self.attempts, first)? != Mode::Plan {
            return Err("部门修正工具缺少正式部门规划阶段".into());
        }
        let failed = prior_failed_plan(self.attempts, api_number, first)?;
        let (previous_binding, initial) = self.initial_tool(number_field(failed, "number")?)?;
        let replaced = &initial["tool"];
        if [replaced, &self.original.request["identity_tool"]]
            .iter()
            .any(|prior| same_tool(prior, tool))
        {
            return Err("部门修正工具与已失败的原工具相同".into());
        }
        let failure = self.failed_attempt(failed, first, previous_binding.as_ref(), &initial)?;
        Ok(json!({
            "format_version":1,"kind":"devex-seed-department-tool-correction",
            "run_manifest":state::manifest_binding(self.run)?,"first_attempt":first,
            "controller":controller,"original_registration":self.original.descriptor,
            "identity_plan":self.original.request["identity_plan"],
            "api_publication":initial["api_publication"],"api_result":self.api["result"],
            "previous_registration":previous_binding,"previous_tool":replaced,
            "failure":failure,"build_request":build_binding,"tool":tool
        }))
    }

    fn initial_tool(&self, failed: u64) -> Result<(Option<Value>, Value)> {
        let initial_path = super::path(self.run);
        let registered = existing(self.backend, &initial_path)?;
        let first = registered.as_ref().map_or(Ok(failed), |(_, receipt)| {
            number_field(receipt, "first_attempt")
        })?;
        if first > failed {
            return Err("原部门工具登记晚于已失败的规划".into());
        }
        let expected = build(
            self.backend,
            self.run,
            self.attempts,
            first,
            self.api,
            self.original,
            self.publication,
        )?;
        let binding = match registered {
            Some((binding, receipt)) if receipt == expected => Some(binding),
            Some(_) => return Err("原部门工具登记不同于其正式 API 和失败历史".into()),
            None => None,
        };
        Ok((binding, expected))
    }

    fn failed_attempt(
        &self,
        failed: &Value,
        first: u64,
        previous: Option<&Value>,
        initial: &Value,
    ) -> Result<Value> {
        let number = number_field(failed, "number")?;
        let error_type = document::text(&failed["error_type"], "前次部门规划失败类型")?;
        let controller = controller(self.backend, self.run, failed)?;
        let failure = common::binding(&self.run.join(format!("failure-{number:04}.json")))?;
        if common::read_bound(self.backend, &failure, "前次部门规划失败")?
            != json!({"format_version":2,"kind":"devex-stage-failure","attempt":number,
                "stage":"seed-runtime","mode":"departments-plan","error_type":error_type,
                "operation_id":"clone.seed-runtime.departments-plan","controller":controller})
        {
            return Err("前次部门规划失败不同于原正式控制器和阶段".into());
        }
        let output = output(self.run, number);
        let child = if present(&output.join("session-launch.json"))? {
            let descriptor = previous.ok_or("已启动的部门规划缺少原后继工具登记")?;
            let request = project(self.original.request.clone(), initial);
            self.recovered_producer(
                number,
                first,
                &output,
                &controller,
                &RegisteredPlan {
                    descriptor,
                    request: &request,
                },
            )?
        } else {
            require_no_child(self.backend, &output)?;
            Value::Null
        };
        Ok(json!({"attempt":number,"controller":controller,"failure":failure,"producer":child}))
    }

    fn recovered_producer(
        &self,
        number: u64,
        first: u64,
        output: &Path,
        controller: &Value,
        previous: &RegisteredPlan<'_>,
    ) -> Result<Value> {
        let launch = common::binding(&output.join("session-launch.json"))?;
        let process = common::binding(&output.join("session-process.json"))?;
        let bridge = common::binding(&output.join("http-failure-1.json"))?;
        old_producer(
            self.backend,
            self.run,
            number,
            controller,
            &launch,
            &process,
            previous,
        )?;
        if common::read_bound(self.backend, &bridge, "原部门初始化桥接失败")?
            != json!({"id":1,"ok":false,"status":null,"error_type":"NativeBridgeFailure"})
            || present(&output.join("authentication.json"))?
        {
            return Err("原部门会话缺少正式首个初始化桥接失败证明".into());
        }
        let recovery = self.recovery(number, first, &process)?;
        Ok(json!({"launch":launch,"process":process,"bridge_failure":bridge,"recovery":recovery}))
    }

    fn recovery(&self, number: u64, first: u64, process: &Value) -> Result<Value> {
        let next = self
            .attempts
            .iter()
            .find(|row| {
                row["stage"] == "seed-runtime" && row["number"].as_u64().is_some_and(|n| n > number)
            })
            .ok_or("前次部门生产者缺少正式会话回收")?;
        let next_number = number_field(next, "number")?;
        let result = &next["result"];
        if next_number >= first
            || next["status"] != "passed"
            || next["mode"] != "recover-session"
            || common::bound_file(self.backend, result, "部门会话回收结果")?
                != state::result_path(self.run, next_number)
        {
            return Err("原部门生产者未在下一个 seed 阶段完成会话回收".into());
        }
        let recovered = common::read_bound(self.backend, result, "部门生产者零写入回收")?;
        if !recovered["already_stopped"].is_boolean()
            || recovered
                != json!({"status":"seed_session_recovered","producer":process,
                "already_stopped":recovered["already_stopped"],"copy_requires_reconciliation":true,
                "remote_writes":0,"restore_qualified":false})
        {
            return Err("原回收结果没有证明同一部门生产者及零远端写入".into());
        }
        Ok(result.clone())
    }
}

fn prior_failed_plan(attempts: &[Value], api: u64, corrected: u64) -> Result<&Value> {
    let previous = attempts
        .iter()
        .rev()
        .find(|row| {
            is_plan(row)
                && row["number"]
                    .as_u64()
                    .is_some_and(|n| api < n && n < corrected)
        })
        .ok_or("部门工具修正缺少已发布 API 之后的前次规划")?;
    if previous["status"] != "failed"
        || !previous["result"].is_null()
        || !previous["finished_at"].is_string()
    {
        return Err("部门工具修正需要前次规划明确失败且没有阶段成功结果".into());
    }
    Ok(previous)
}

fn same_tool(previous: &Value, tool: &Value) -> bool {
    previous["path"] == tool["path"]
        || matches!((previous["sha256"].as_str(),tool["sha256"].as_str()),
            (Some(a),Some(b)) if a.eq_ignore_ascii_case(b))
}

fn require_no_plan(run: &Path) -> Result<()> {
    for name in ["department-plan.json", "departments"] {
        if present(&run.join("seed-runtime").join(name))? {
            return Err("部门修正发现已存在的规划、身份前提或写入意图，保留现场".into());
        }
    }
    Ok(())
}

fn require_no_child(backend: &Path, output: &Path) -> Result<()> {
    if present(output)? {
        common::local_directory(backend, output, "前次部门规划输出")?;
        for file in fs::read_dir(output)? {
            let name = file?.file_name();
            let name = name.to_str().ok_or("部门规划输出名称无效")?;
            if name == "pacing"
                || name == "authentication.json"
                || name.starts_with("session")
                || name.starts_with("http-")
                || name.starts_with("department-")
                || name.ends_with("-before.json")
            {
                return Err("未启动的部门规划出现了生产者或部门业务收据，保留现场".into());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[test]
fn department_correction_uses_latest_failed_plan_after_api() -> Result<()> {
    let mut attempts = [51, 56, 58].map(|number| {
        json!({"stage":"seed-runtime","mode":"departments-plan","number":number,
            "status":"failed","finished_at":"2026-11-26T00:00:00Z","result":null})
    });
    assert!(prior_failed_plan(&attempts, 55, 56).is_err());
    assert_eq!(prior_failed_plan(&attempts, 55, 57)?["number"], 56);
    attempts[2]["status"] = json!("passed");
    assert!(prior_failed_plan(&attempts, 55, 59).is_err());
    Ok(())
}

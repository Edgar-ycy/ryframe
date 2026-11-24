//! 启动收据只认可原登记和同一生命周期中的运行代次。
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::super::{
    dataset_authority::{check, commands, inputs::Inputs, path, registration, schema},
    generation,
};
use super::{canonical_input, registered_path};
use crate::{
    Result,
    local_test_path::LocalTestPathKind::ExistingFile,
    recovery::inputs::{
        document::{self, Document},
        domain, reference,
    },
};

#[derive(Clone, Copy)]
pub(in crate::recovery::runtime) enum Operation {
    Bind,
    Verify,
}

impl Operation {
    pub(in crate::recovery::runtime) fn describe(
        self,
        registration: &str,
        target: &str,
        number: u64,
    ) -> String {
        let name = match self {
            Self::Bind => "runtime-bind",
            Self::Verify => "runtime-verify",
        };
        format!("{name}:{registration}:{target}:{number}")
    }

    pub(in crate::recovery::runtime) fn drift(self) -> &'static str {
        match self {
            Self::Bind => "恢复运行 bind 的登记输入或运行代次发生变化",
            Self::Verify => "恢复运行在线核验的登记输入或运行代次发生变化",
        }
    }
}

pub(in crate::recovery::runtime) struct Control {
    pub(in crate::recovery::runtime) launch: Document,
    pub(in crate::recovery::runtime) directory: PathBuf,
    pub(in crate::recovery::runtime) operation: String,
    pub(in crate::recovery::runtime) snapshot: Value,
}

impl Control {
    pub(in crate::recovery::runtime) fn same_generation_as(&self, previous: &Self) -> bool {
        self.snapshot == previous.snapshot && self.operation == previous.operation
    }

    pub(in crate::recovery::runtime) fn read(
        inputs: &mut Inputs,
        launch_path: &Path,
        operation: Operation,
    ) -> Result<Self> {
        let launch = inputs.read(&canonical_input(launch_path, ExistingFile)?)?;
        let launch_dir = path(&launch.value["runtime_directory"])?;
        registered_path(
            &launch.path,
            &launch_dir.join("runtime-launch.json"),
            ExistingFile,
        )?;
        let request = &launch.value["request"];
        let binding = &request["registration"];
        let number = document::integer(&launch.value["generation"], "恢复启动代次")?;
        let commands = commands::verify_direct(request, inputs)?;
        generation::request(&json!({
            "action":"validate-launch", "root":launch_dir, "binding":binding,
            "value":launch.value, "previous":null, "commands":commands, "number":number,
        }))?;
        let registered = inputs.read(&path(&binding["registration"]["path"])?)?;
        let target = inputs.bound(&binding["target_plan"])?;
        let reference = registration::read_reference(inputs, &launch.value, &target, &registered)?;
        let (directory, _) = runtime_target(&reference.value, &target.value)?;
        let lifecycle = inputs.read(&directory.join("lifecycle.json"))?;
        let launch = running(
            inputs,
            &directory,
            binding,
            &launch,
            &lifecycle.value,
            number,
        )?;
        let operation = operation.describe(&registered.sha256(), &target.sha256(), number);
        let snapshot = json!({
            "directory":directory, "binding":binding, "launch":document::binding(&launch),
            "registration":document::binding(&registered), "target_plan":document::binding(&target),
            "reference_plan":document::binding(&reference), "lifecycle":document::binding(&lifecycle),
        });
        Ok(Self {
            launch,
            directory,
            operation,
            snapshot,
        })
    }
}

pub(in crate::recovery::runtime) fn runtime_target(
    reference: &Value,
    target: &Value,
) -> Result<(PathBuf, Value)> {
    let reference = runtime_reference(reference, target)?;
    let selected = &reference["target"];
    let product = &target["product_plan"];
    for key in ["fresh_target", "maintenance_execution", "product_execution"] {
        check(target.get(key).is_some(), "恢复登记缺少正式目标输入")?;
    }
    let endpoints = json!({
        "api":product["api_ready_url"], "worker":product["worker_ready_url"],
        "frontend":selected["frontend_url"],
    });
    schema::endpoints(&endpoints)?;
    domain::hash_40(&product["frontend_sha"], "恢复登记前端提交")?;
    check(
        document::text(&target["target_side"], "恢复目标侧别")?
            == document::text(&reference["target_side"], "恢复参考侧别")?
            && document::text(&product["scope_id"], "恢复产品 scope")?
                == document::text(&selected["scope_id"], "恢复参考 scope")?,
        "恢复登记的目标侧或 scope 与参考计划不同",
    )?;
    Ok((path(&selected["runtime_dir"])?, endpoints))
}

fn runtime_reference<'a>(reference: &'a Value, target: &Value) -> Result<&'a Value> {
    if !reference::is_independent_b1(reference) {
        return Ok(reference);
    }
    document::exact(
        reference,
        &[
            "format_version",
            "kind",
            "reference_plan",
            "arm_input",
            "source_export",
        ],
        "独立 B1 原参考计划封装",
    )?;
    let plan = &reference["reference_plan"];
    check(
        reference["format_version"] == 1
            && target["kind"] == "restore-reference-target-plan"
            && target["reference_plan_sha256"] == document::hash(reference)?
            && plan["target_side"] == "candidate",
        "恢复运行必须使用目标绑定的原独立 B1 计划封装",
    )?;
    Ok(plan)
}

fn running(
    inputs: &mut Inputs,
    directory: &Path,
    binding: &Value,
    launch: &Document,
    state: &Value,
    number: u64,
) -> Result<Document> {
    let generations = document::array(&state["generations"], "恢复运行代次")?;
    let commands = generations
        .iter()
        .map(|generation| commands::verify_direct(&generation["request"], inputs))
        .collect::<Result<Vec<_>>>()?;
    generation::request(&json!({
        "action":"validate-state", "root":directory, "binding":binding, "value":state,
        "previous":null, "commands":commands, "number":1,
    }))?;
    let generation = number
        .checked_sub(1)
        .and_then(|index| usize::try_from(index).ok())
        .and_then(|index| generations.get(index))
        .ok_or("恢复启动代次不存在")?;
    check(
        generation["number"] == number
            && generation["status"] == "running"
            && generation["request"] == launch.value["request"],
        "恢复运行 bind 只接受同一生命周期的运行中代次",
    )?;
    let descriptor = &generation["launch"];
    let registered = path(&descriptor["path"])?;
    registered_path(&launch.path, &registered, ExistingFile)?;
    let mut document = inputs.read(&registered)?;
    // 读取窗口可能复用了规范路径的同一文件，输出仍保留原登记的描述路径。
    document.path = registered;
    check(
        document::binding(&document) == *descriptor && document.raw == launch.raw,
        "恢复运行启动收据与已登记代次的路径、摘要或原字节不同",
    )?;
    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_runtime_target_checks_the_original_envelope_before_using_its_plan() -> Result<()>
    {
        let work = crate::workspace::root_dir().join(".local-tests/runtime-case");
        let plan = json!({
            "target_side":"candidate",
            "target":{"runtime_dir":work,"scope_id":"test-arm","frontend_url":"http://127.0.0.1:4123"}
        });
        let mut reference = json!({
            "format_version":1,"kind":"restore-reference-independent-b1-plan",
            "reference_plan":plan,"arm_input":{},"source_export":{}
        });
        let target = json!({
            "kind":"restore-reference-target-plan","target_side":"candidate",
            "reference_plan_sha256":document::hash(&reference)?,
            "product_plan":{"scope_id":"test-arm","frontend_sha":"b".repeat(40),
                "api_ready_url":"http://127.0.0.1:4121/readyz","worker_ready_url":"http://127.0.0.1:4122/readyz"},
            "fresh_target":{}, "maintenance_execution":{}, "product_execution":{}
        });
        let (directory, endpoints) = runtime_target(&reference, &target)?;
        assert_eq!(directory, work);
        assert_eq!(endpoints["frontend"], "http://127.0.0.1:4123");
        reference["source_export"] = json!({"different":true});
        assert!(runtime_target(&reference, &target).is_err());
        Ok(())
    }
}

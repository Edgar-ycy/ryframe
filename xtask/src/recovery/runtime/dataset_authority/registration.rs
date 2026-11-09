//! 登记绑定必须来自 launch，不能采用备份的原参考计划替代恢复目标。
use super::{check, inputs::Inputs, path};
use crate::{
    Result,
    recovery::{
        clone::native::json as strict,
        inputs::document::{self, Document},
    },
};
use serde_json::Value;

pub(super) fn reference(
    inputs: &mut Inputs,
    launch: &Value,
    target: &Document,
) -> Result<Document> {
    let registration = inputs.bound(&launch["request"]["registration"]["registration"])?;
    read_reference(inputs, launch, target, &registration)
}

pub(in crate::recovery::runtime) fn read_reference(
    inputs: &mut Inputs,
    launch: &Value,
    target: &Document,
    registration: &Document,
) -> Result<Document> {
    read_binding(
        inputs,
        &launch["request"]["registration"],
        target,
        registration,
    )
}

pub(in crate::recovery::runtime) fn read_binding(
    inputs: &mut Inputs,
    binding: &Value,
    target: &Document,
    registration: &Document,
) -> Result<Document> {
    strict::exact(binding, &["registration", "target_plan"], "恢复登记绑定")?;
    check(
        binding["target_plan"] == document::binding(target)
            && binding["registration"] == document::binding(registration),
        "恢复代次未绑定显式登记和目标计划",
    )?;
    let value = &registration.value;
    strict::exact(
        value,
        &[
            "format_version",
            "kind",
            "reference_plan",
            "target_plan",
            "observation",
            "remote_writes",
        ],
        "恢复运行登记",
    )?;
    check(
        value["format_version"] == 1
            && value["kind"] == "restore-runtime-registration"
            && value["remote_writes"].as_u64() == Some(0)
            && value["target_plan"] == binding["target_plan"],
        "恢复运行登记类型、写入声明或目标绑定不同",
    )?;
    let observation = &value["observation"];
    strict::exact(observation, &["runtime", "ports_idle"], "登记零进程观察")?;
    let runtime = &observation["runtime"];
    strict::exact(
        runtime,
        &["exists", "entries", "anchor"],
        "登记运行目录观察",
    )?;
    strict::exact(
        &runtime["anchor"],
        &["path", "device", "inode", "created_ns"],
        "登记运行目录身份",
    )?;
    path(&runtime["anchor"]["path"])?;
    check(
        runtime["exists"].is_boolean()
            && runtime["entries"] == serde_json::json!([])
            && observation["ports_idle"] == serde_json::json!(["api", "worker", "frontend"])
            && ["device", "inode", "created_ns"]
                .iter()
                .all(|key| runtime["anchor"][key].is_i64() || runtime["anchor"][key].is_u64()),
        "恢复运行登记未记录空目录和三端空闲",
    )?;
    inputs.bound(&value["target_plan"])?;
    inputs.bound(&value["reference_plan"])
}

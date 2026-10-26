//! 前端构建证明的原生只读权威；源码、配置和产物完整核验后才返回。
use super::clone::native::source;
use crate::Result;
use serde_json::{Value, json};
use std::{fmt, path::Path};
#[path = "frontend_build/configuration.rs"]
mod configuration;
#[path = "frontend_build/files.rs"]
mod files;
#[path = "frontend_build/model.rs"]
mod model;
#[path = "frontend_build/protocol.rs"]
pub(crate) mod protocol;
#[cfg(test)]
#[path = "../../tests/unit/recovery/frontend_build.rs"]
mod tests;

#[derive(Debug)]
struct Rejected(&'static str);
impl fmt::Display for Rejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for Rejected {}
fn fail(reason: &'static str) -> Box<dyn std::error::Error> {
    Box::new(Rejected(reason))
}
fn reason(error: &(dyn std::error::Error + 'static)) -> &'static str {
    error
        .downcast_ref::<Rejected>()
        .map(|error| error.0)
        .unwrap_or("来源、文件身份或绑定复核失败")
}
pub(crate) struct Proof {
    pub(crate) inventory: Value,
    pub(crate) receipt: Value,
    root: std::path::PathBuf,
    receipt_file: files::StableFile,
    inputs: configuration::Inputs,
    artifacts: files::Artifacts,
}
impl Proof {
    pub(crate) fn verify(&self) -> Result<()> {
        self.receipt_file.verify()?;
        self.inputs.verify()?;
        self.artifacts.verify()?;
        let repeated = configuration::capture(
            &self.root,
            model::text(&self.inputs.configuration["environment"])?,
        )?;
        if repeated.configuration != self.inputs.configuration
            || repeated.identity != self.inputs.identity
            || files::artifacts(&self.root)?.files != self.artifacts.files
            || source::inventory_value(&source::capture(&self.root)?) != self.inventory
        {
            return Err(fail("核验期间源码、配置、身份或完整 dist 发生漂移"));
        }
        self.receipt_file.verify()?;
        self.inputs.verify()?;
        self.artifacts.verify()
    }
    fn summary(&self) -> Value {
        json!({"configuration":self.inputs.configuration,"identity":self.inputs.identity,"files":self.artifacts.files})
    }
}
pub(crate) fn validate(
    root: &Path,
    inventory: Option<&Value>,
    expected: Option<&str>,
) -> Result<Proof> {
    let root = files::directory(root, root)?;
    let inventory = match inventory {
        Some(value) => value.clone(),
        None => source::inventory_value(&source::capture(&root)?),
    };
    let observed = source::validate_inventory(&inventory)
        .map_err(|_| fail("完整来源清单必须包含合法 source、files 和 guard；检查采集入口"))?;
    let (receipt_file, receipt) = files::document(&root.join("dist").join(files::RECEIPT))?;
    model::receipt(&receipt)?;
    let inputs = configuration::verify(&root, &receipt["build"], expected)?;
    let commit = model::text(&inputs.identity["frontend_commit"])?;
    if commit != "development"
        && (!observed.source.snapshot.clean || observed.source.snapshot.head != commit)
    {
        return Err(fail("正式 frontend_commit 必须匹配当前精确干净源码 SHA"));
    }
    let domains = source::protocol::request(
        &json!({"action":"domains","repository":"frontend","inventory":inventory}),
    )?;
    let artifacts = files::artifacts(&root)?;
    if receipt["sources"] != domains
        || model::normalized_files(&receipt["files"])? != artifacts.files
    {
        return Err(fail("v3 收据不同于当前来源三域或完整产物"));
    }
    let proof = Proof {
        inventory,
        receipt,
        root,
        receipt_file,
        inputs,
        artifacts,
    };
    proof.verify()?;
    Ok(proof)
}

/// 现有恢复只读协议的适配；所有规则直接使用同一原生模型与文件验证。
/// 返回的是本次调用核验的快照事实，不跨 IPC 持有保护；调用者须在使用窗口末端
/// 通过同一权威重新核验收据、inputs 与文件事实，不能把此返回值当成持续持有句柄。
pub(crate) fn recovery_request(arguments: &Value) -> Result<Value> {
    model::exact(
        arguments,
        &["action", "root", "value", "expected_head", "inventory"],
    )?;
    let root = Path::new(model::text(&arguments["root"])?);
    let expected = if arguments["expected_head"].is_null() {
        None
    } else {
        Some(model::text(&arguments["expected_head"])?)
    };
    match model::text(&arguments["action"])? {
        "context" => {
            model::context(&arguments["value"])?;
            Ok(arguments["value"].clone())
        }
        "receipt" => {
            model::receipt(&arguments["value"])?;
            Ok(arguments["value"].clone())
        }
        "capture-inputs" => {
            let inputs = configuration::capture(root, model::text(&arguments["value"])?)?;
            inputs.verify()?;
            inputs.facts()
        }
        "inputs" => {
            let inputs = configuration::verify(root, &arguments["value"], expected)?;
            inputs.verify()?;
            inputs.facts()
        }
        "artifacts" => {
            let artifacts = files::artifacts(root)?;
            artifacts.verify()?;
            if files::artifacts(root)?.files != artifacts.files {
                return Err(fail("dist 完整文件集合变化"));
            }
            artifacts.verify()?;
            Ok(artifacts.files)
        }
        "validate" => {
            let inventory = (!arguments["inventory"].is_null()).then_some(&arguments["inventory"]);
            let proof = validate(root, inventory, expected)?;
            proof.verify()?;
            Ok(json!({"inventory":proof.inventory,"receipt":proof.receipt}))
        }
        _ => Err(fail("未知前端证明校验操作")),
    }
}

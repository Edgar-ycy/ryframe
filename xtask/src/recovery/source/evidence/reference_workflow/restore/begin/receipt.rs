//! 开始登记的写入意图、进程结果与对应的运行记录各自独占发布。
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    BeginInputs, Document, Inputs, Result, TargetSnapshot, VerifiedCopy, begin_proof, document,
    expected_images, native, path, text,
};
use native::{export::transport::CommandOutput, resources::common};

pub(super) fn directory(inputs: &BeginInputs<'_>) -> Result<PathBuf> {
    directory_for(inputs.plan, &inputs.target.value)
}

pub(super) fn directory_for(plan: &Value, target: &Value) -> Result<PathBuf> {
    let side = text(&target["target_side"])?;
    if !matches!(side, "base" | "candidate") || plan["target_side"] != side {
        return Err("开始登记必须绑定同一明确恢复目标侧".into());
    }
    Ok(path(&plan["work_dir"])?.join(format!("restore-begin-{side}")))
}

fn bindings(inputs: &BeginInputs<'_>) -> Value {
    json!({
        "reference_plan":document::binding(inputs.reference),
        "target_plan":document::binding(inputs.target),
        "product_plan":document::binding(inputs.product_plan),
        "backup_receipt":document::binding(inputs.backup_receipt),
        "manifest":document::binding(inputs.manifest),
        "runtime_registration":document::binding(inputs.registration),
    })
}

pub(super) struct Intent {
    pub(super) preimage: Document,
    image: Document,
    intent: Document,
    protection: Inputs,
}

pub(super) struct Evidence<'a> {
    pub(super) image: &'a Value,
    pub(super) proof: &'a Value,
    pub(super) copy: Option<&'a VerifiedCopy>,
    pub(super) product_plan: &'a Document,
    pub(super) work_binding: Option<&'a Value>,
}

impl Intent {
    pub(super) fn publish(
        backend: &Path,
        directory: &Path,
        inputs: &BeginInputs<'_>,
        output: &Path,
        maintenance: &Value,
        evidence: Evidence<'_>,
    ) -> Result<Self> {
        let image = publish(
            backend,
            &directory.join("resource-image.json"),
            evidence.image,
        )?;
        let preimage = publish(backend, &directory.join("preimage.json"), evidence.proof)?;
        let mut value = json!({
            "format_version":1, "kind":"reference-restore-begin-intent",
            "bindings":bindings(inputs), "preimage":document::binding(&preimage),
            "resource_image":document::binding(&image), "maintenance":maintenance, "record_path":output,
        });
        let mut paths = vec![preimage.path.clone(), image.path.clone()];
        if let Some(binding) = evidence.work_binding {
            value["reference_work"] = binding.clone();
        }
        if let Some(copy) = evidence.copy {
            value["negative_copy"] = json!({
                "id":copy.manifest.value["id"], "original_copy_receipt":document::binding(&copy.receipt),
                "manifest":document::binding(&copy.manifest), "execution_plan":document::binding(evidence.product_plan),
            });
            paths.extend([
                copy.receipt.path.clone(),
                copy.manifest.path.clone(),
                evidence.product_plan.path.clone(),
            ]);
        }
        let intent = publish(backend, &directory.join("intent.json"), &value)?;
        paths.push(intent.path.clone());
        let protection = Inputs::open(paths)?;
        let current = Self {
            preimage,
            image,
            intent,
            protection,
        };
        current.check()?;
        Ok(current)
    }

    pub(super) fn check(&self) -> Result<()> {
        self.protection.check()?;
        self.intent.assert_unchanged()?;
        self.image.assert_unchanged()?;
        self.preimage.assert_unchanged()
    }

    pub(super) fn finish(&self, backend: &Path, record: &Document) -> Result<Value> {
        let directory = self.intent.path.parent().ok_or("开始登记意图缺少目录")?;
        self.check()?;
        let command = document::read(&directory.join("command.json"), backend, "开始登记命令结果")?;
        check_command(&command.value)?;
        let mut outcome = json!({
            "format_version":1, "kind":"reference-restore-begin-result", "status":"completed",
            "intent":document::binding(&self.intent), "command":document::binding(&command),
            "record":document::binding(record),
        });
        record.assert_unchanged()?;
        command.assert_unchanged()?;
        let negative = self.intent.value.get("negative_copy").is_some();
        if let Some(binding) = self.intent.value.get("reference_work") {
            outcome["reference_work"] = binding.clone();
        }
        if negative {
            let registration = document::read(
                &directory.join("copy-registration.json"),
                backend,
                "独立副本登记结果",
            )?;
            check_command(&registration.value)?;
            outcome["copy_registration"] = document::binding(&registration);
            registration.assert_unchanged()?;
        }
        self.check()?;
        let receipt = publish(backend, &directory.join("result.json"), &outcome)?;
        Ok(json!({
            "status":if negative {"negative_restore_begin_registered"} else {"restore_begin_registered"},
            "record":document::binding(record),
            "receipt":document::binding(&receipt),
        }))
    }
}

pub(super) fn record_unconfirmed(backend: &Path, directory: &Path) -> Result<()> {
    let filename = directory.join("intent.json");
    if !filename.try_exists()? {
        publish(
            backend,
            &directory.join("preimage-failure.json"),
            &json!({
                "format_version":1, "kind":"reference-restore-begin-result",
                "status":"preimage_rejected", "maintenance_invoked":false,
            }),
        )?;
        return Ok(());
    }
    let intent = document::read(&filename, backend, "开始登记原意图")?;
    let failure = json!({
        "format_version":1, "kind":"reference-restore-begin-result", "status":"unconfirmed",
        "intent":document::binding(&intent),
    });
    publish(backend, &directory.join("failure.json"), &failure)?;
    Ok(())
}

pub(super) fn record_command(path: &Path, result: &Result<CommandOutput>) -> Result<()> {
    let describe =
        |bytes: &[u8]| json!({"bytes":bytes.len(),"sha256":hex::encode(Sha256::digest(bytes))});
    let status = match result {
        Ok(output) => json!({
            "format_version":1, "kind":"reference-restore-begin-command",
            "exit_code":output.status.code(), "success":output.status.success(),
            "stdout":describe(&output.stdout), "stderr":describe(&output.stderr),
            "failure":(!output.status.success()).then(|| failure_label(&output.stderr)),
        }),
        Err(_) => json!({
            "format_version":1, "kind":"reference-restore-begin-command",
            "exit_code":null, "success":false, "stdout":null, "stderr":null,
            "failure":"TransportUnconfirmed",
        }),
    };
    native::json::write_new(path, &status)
}

fn failure_label(stderr: &[u8]) -> &'static str {
    let recognized = [
        ("未找到有效备份集", "BackupNotRegistered"),
        (
            "恢复目标当前完整库清单与绑定的初始化前像不同",
            "DatabasePreimageDrift",
        ),
        (
            "恢复初始化前像的版本或产品计划摘要不同",
            "PreimagePlanMismatch",
        ),
        ("恢复记录写后", "OutputVerificationUnconfirmed"),
        ("恢复记录临时文件", "OutputPublicationUnconfirmed"),
    ];
    let text = String::from_utf8_lossy(stderr);
    recognized
        .iter()
        .find_map(|(message, kind)| text.contains(message).then_some(*kind))
        .unwrap_or("MaintenanceExitUnconfirmed")
}

#[cfg(test)]
#[test]
fn maintenance_exit_preserves_only_a_fixed_failure_classification() {
    assert_eq!(
        failure_label("未找到有效备份集".as_bytes()),
        "BackupNotRegistered"
    );
    assert_eq!(
        failure_label(b"arbitrary local diagnostic"),
        "MaintenanceExitUnconfirmed"
    );
}

pub(super) fn check_command(value: &Value) -> Result<()> {
    document::exact(
        value,
        &[
            "format_version",
            "kind",
            "exit_code",
            "success",
            "stdout",
            "stderr",
            "failure",
        ],
        "开始登记命令结果",
    )?;
    if value["format_version"] != 1
        || value["kind"] != "reference-restore-begin-command"
        || value["success"] != true
        || value["exit_code"] != 0
        || !value["failure"].is_null()
    {
        return Err("参考恢复缺少成功的维护程序开始登记命令结果".into());
    }
    for field in ["stdout", "stderr"] {
        document::exact(&value[field], &["bytes", "sha256"], "开始登记命令输出摘要")?;
        document::integer(&value[field]["bytes"], "开始登记命令输出字节数")?;
        document::digest(&value[field]["sha256"], "开始登记命令输出摘要")?;
    }
    Ok(())
}

pub(in super::super) fn verify_record(
    backend: &Path,
    snapshot: &TargetSnapshot,
    record: &Document,
) -> Result<Document> {
    let directory = directory(&BeginInputs::from(snapshot))?;
    if directory.join("failure.json").try_exists()? {
        return Err("开始登记曾留下未确认结果，必须先只读核对原登记库".into());
    }
    let receipt = document::read(&directory.join("result.json"), backend, "持锁开始登记结果")?;
    let value = &receipt.value;
    check_result(value, record, false)?;
    let intent = bound(backend, &directory.join("intent.json"), &value["intent"])?;
    let command = bound(backend, &directory.join("command.json"), &value["command"])?;
    check_command(&command.value)?;
    verify_intent(
        backend,
        &directory,
        &BeginInputs::from(snapshot),
        record,
        &intent,
        None,
    )?;
    command.assert_unchanged()?;
    intent.assert_unchanged()?;
    receipt.assert_unchanged()?;
    Ok(receipt)
}

pub(super) fn check_result(value: &Value, record: &Document, independent: bool) -> Result<()> {
    let fields = [
        "format_version",
        "kind",
        "status",
        "intent",
        "command",
        "record",
        "reference_work",
    ];
    document::exact(
        value,
        &fields[..if independent {
            fields.len()
        } else {
            fields.len() - 1
        }],
        "开始登记结果",
    )?;
    if value["format_version"] != 1
        || value["kind"] != "reference-restore-begin-result"
        || value["status"] != "completed"
        || value["record"] != document::binding(record)
    {
        return Err("参考恢复必须消费同一持锁开始登记发布的产品运行记录".into());
    }
    Ok(())
}

pub(super) fn check_intent(value: &Value, record: &Document, work: Option<&Value>) -> Result<()> {
    let fields = [
        "format_version",
        "kind",
        "bindings",
        "preimage",
        "resource_image",
        "maintenance",
        "record_path",
        "reference_work",
    ];
    document::exact(
        value,
        &fields[..if work.is_some() {
            fields.len()
        } else {
            fields.len() - 1
        }],
        "开始登记意图",
    )?;
    if value["format_version"] != 1
        || value["kind"] != "reference-restore-begin-intent"
        || path(&value["record_path"])? != record.path
        || work.is_some_and(|work| value["reference_work"] != *work)
    {
        return Err("开始登记意图的原备份、计划、运行登记或产物路径不同".into());
    }
    Ok(())
}

pub(super) struct Images {
    pub(super) preimage: Document,
    pub(super) resource: Document,
}

pub(super) fn verify_intent(
    backend: &Path,
    directory: &Path,
    inputs: &BeginInputs<'_>,
    record: &Document,
    intent: &Document,
    work: Option<&Value>,
) -> Result<Images> {
    let value = &intent.value;
    check_intent(value, record, work)?;
    if value["bindings"] != bindings(inputs) {
        return Err("开始登记意图的原备份、计划、运行登记或产物路径不同".into());
    }
    let preimage = bound(
        backend,
        &directory.join("preimage.json"),
        &value["preimage"],
    )?;
    let image = bound(
        backend,
        &directory.join("resource-image.json"),
        &value["resource_image"],
    )?;
    let descriptor = &inputs.target.value["fresh_target"]["initialized"];
    let (_, initialized) = document::bound_file(backend, descriptor, "原目标初始化")?;
    let (before, _) =
        expected_images::expected_images(inputs.plan, &inputs.manifest.value, &initialized.value)?;
    if image.value != before
        || preimage.value
            != begin_proof::build(
                &inputs.product_plan.value,
                &inputs.target.sha256(),
                &initialized.sha256(),
                &before,
            )?
    {
        return Err("开始登记前像不等于本次恢复绑定的原初始化".into());
    }
    verify_maintenance(&value["maintenance"])?;
    preimage.assert_unchanged()?;
    image.assert_unchanged()?;
    initialized.assert_unchanged()?;
    Ok(Images {
        preimage,
        resource: image,
    })
}

fn verify_maintenance(maintenance: &Value) -> Result<()> {
    document::exact(
        maintenance,
        &["root", "build", "executable", "source"],
        "开始登记维护构建",
    )?;
    let root = path(&maintenance["root"])?;
    let build = common::bound_file(root, &maintenance["build"], "开始登记维护构建")?;
    let tool = common::bound_file(root, &maintenance["executable"], "开始登记维护程序")?;
    let built = native::maintenance::verify_evidence(root, &build, None)?;
    let artifact = &built["artifacts"]["tenant-data"];
    if built["source"] != maintenance["source"]
        || path(&artifact["executable"])? != tool
        || artifact["bytes"] != maintenance["executable"]["bytes"]
        || artifact["sha256"] != maintenance["executable"]["sha256"]
    {
        return Err("开始登记维护程序来源和已登记产物不同".into());
    }
    Ok(())
}

pub(super) fn bound(backend: &Path, path: &Path, binding: &Value) -> Result<Document> {
    let (_, file) = document::bound_file(backend, binding, "开始登记阶段原文件")?;
    if file.path != path {
        return Err("开始登记阶段原文件路径不同".into());
    }
    Ok(file)
}

fn publish(backend: &Path, path: &Path, value: &Value) -> Result<Document> {
    native::json::publish_new(path, value, "正式开始登记收据")?;
    let document = document::read(path, backend, "正式开始登记收据")?;
    if document.value != *value {
        return Err("开始登记收据写后回读不同".into());
    }
    Ok(document)
}

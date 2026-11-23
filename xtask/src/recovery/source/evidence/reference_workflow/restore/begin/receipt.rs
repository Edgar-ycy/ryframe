//! 开始登记的写入意图、进程结果与对应的运行记录各自独占发布。
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    Document, Inputs, Result, TargetSnapshot, begin_proof, document, expected_images, native, path,
    text,
};
use native::{export::transport::CommandOutput, resources::common};

pub(super) fn directory(snapshot: &TargetSnapshot) -> Result<PathBuf> {
    let side = text(&snapshot.target.value["target_side"])?;
    if !matches!(side, "base" | "candidate")
        || snapshot.chain.reference.value["target_side"] != side
    {
        return Err("开始登记必须绑定同一明确恢复目标侧".into());
    }
    Ok(path(&snapshot.chain.reference.value["work_dir"])?.join(format!("restore-begin-{side}")))
}

fn bindings(snapshot: &TargetSnapshot) -> Value {
    json!({
        "reference_plan":document::binding(&snapshot.chain.reference),
        "target_plan":document::binding(&snapshot.target),
        "product_plan":document::binding(&snapshot.product_plan),
        "backup_receipt":document::binding(&snapshot.chain.backup_document),
        "manifest":document::binding(&snapshot.manifest),
        "runtime_registration":document::binding(&snapshot.registration),
    })
}

pub(super) struct Intent {
    pub(super) preimage: Document,
    image: Document,
    intent: Document,
    protection: Inputs,
}

impl Intent {
    pub(super) fn publish(
        backend: &Path,
        directory: &Path,
        snapshot: &TargetSnapshot,
        output: &Path,
        maintenance: &Value,
        image: &Value,
        proof: &Value,
    ) -> Result<Self> {
        let image = publish(backend, &directory.join("resource-image.json"), image)?;
        let preimage = publish(backend, &directory.join("preimage.json"), proof)?;
        let value = json!({
            "format_version":1, "kind":"reference-restore-begin-intent",
            "bindings":bindings(snapshot), "preimage":document::binding(&preimage),
            "resource_image":document::binding(&image), "maintenance":maintenance, "record_path":output,
        });
        let intent = publish(backend, &directory.join("intent.json"), &value)?;
        let protection = Inputs::open([
            preimage.path.clone(),
            image.path.clone(),
            intent.path.clone(),
        ])?;
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
        let outcome = json!({
            "format_version":1, "kind":"reference-restore-begin-result", "status":"completed",
            "intent":document::binding(&self.intent), "command":document::binding(&command),
            "record":document::binding(record),
        });
        record.assert_unchanged()?;
        command.assert_unchanged()?;
        self.check()?;
        let receipt = publish(backend, &directory.join("result.json"), &outcome)?;
        Ok(json!({
            "status":"restore_begin_registered", "record":document::binding(record),
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

pub(super) fn record_command(directory: &Path, result: &Result<CommandOutput>) -> Result<()> {
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
    native::json::write_new(&directory.join("command.json"), &status)
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

fn check_command(value: &Value) -> Result<()> {
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
    let directory = directory(snapshot)?;
    if directory.join("failure.json").try_exists()? {
        return Err("开始登记曾留下未确认结果，必须先只读核对原登记库".into());
    }
    let receipt = document::read(&directory.join("result.json"), backend, "持锁开始登记结果")?;
    let value = &receipt.value;
    document::exact(
        value,
        &[
            "format_version",
            "kind",
            "status",
            "intent",
            "command",
            "record",
        ],
        "开始登记结果",
    )?;
    if value["format_version"] != 1
        || value["kind"] != "reference-restore-begin-result"
        || value["status"] != "completed"
        || value["record"] != document::binding(record)
    {
        return Err("参考恢复必须消费同一持锁开始登记发布的产品运行记录".into());
    }
    let intent = bound(backend, &directory.join("intent.json"), &value["intent"])?;
    let command = bound(backend, &directory.join("command.json"), &value["command"])?;
    check_command(&command.value)?;
    verify_intent(backend, &directory, snapshot, record, &intent)?;
    command.assert_unchanged()?;
    intent.assert_unchanged()?;
    receipt.assert_unchanged()?;
    Ok(receipt)
}

fn verify_intent(
    backend: &Path,
    directory: &Path,
    snapshot: &TargetSnapshot,
    record: &Document,
    intent: &Document,
) -> Result<()> {
    let value = &intent.value;
    document::exact(
        value,
        &[
            "format_version",
            "kind",
            "bindings",
            "preimage",
            "resource_image",
            "maintenance",
            "record_path",
        ],
        "开始登记意图",
    )?;
    if value["format_version"] != 1
        || value["kind"] != "reference-restore-begin-intent"
        || value["bindings"] != bindings(snapshot)
        || path(&value["record_path"])? != record.path
    {
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
    let descriptor = &snapshot.target.value["fresh_target"]["initialized"];
    let (_, initialized) = document::bound_file(backend, descriptor, "原目标初始化")?;
    let (before, _) = expected_images::expected_images(
        &snapshot.chain.reference.value,
        &snapshot.manifest.value,
        &initialized.value,
    )?;
    if image.value != before
        || preimage.value
            != begin_proof::build(
                &snapshot.product_plan.value,
                &snapshot.target.sha256(),
                &initialized.sha256(),
                &before,
            )?
    {
        return Err("开始登记前像不等于本次恢复绑定的原初始化".into());
    }
    let maintenance = &value["maintenance"];
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
    preimage.assert_unchanged()?;
    image.assert_unchanged()?;
    initialized.assert_unchanged()
}

fn bound(backend: &Path, path: &Path, binding: &Value) -> Result<Document> {
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

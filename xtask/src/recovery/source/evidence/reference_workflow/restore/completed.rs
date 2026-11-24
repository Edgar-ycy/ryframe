//! 已完成的独立 B1 外部恢复证据，只借用原文件和目录保护。
use std::{fs, io::ErrorKind, path::Path};

use serde_json::{Value, json};

use super::super::local::OriginalWork;
use super::{
    begin::{BeginInputs, HeldBeginImages, SealedBegin, independent_plan},
    document, expected_images, native, path,
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::{
        clone::native::target_verify::guard::Inputs,
        inputs::{document::Document, domain},
        runtime::dataset_authority::reads,
    },
};

#[path = "completed/observations.rs"]
mod observations;
use observations::{ObjectPlan, check_image, read_image};

const BINDINGS: [&str; 6] = [
    "target_side",
    "target_plan",
    "backup_receipt",
    "record",
    "begin_receipt",
    "runtime_registration",
];

pub(in crate::recovery) struct CompletedIndependentRestore<'a> {
    reference: &'a Document,
    target: &'a Document,
    backup: &'a Document,
    plan: &'a Value,
    work: OriginalWork,
    completed: Document,
    running: Document,
    registration: Document,
    product: Document,
    manifest: Document,
    begin: SealedBegin,
    begin_images: HeldBeginImages,
    before: Document,
    after: Document,
    files: Inputs,
}

impl<'a> CompletedIndependentRestore<'a> {
    pub(in crate::recovery) fn read(
        backend: &Path,
        reference: &'a Document,
        target: &'a Document,
        backup_receipt: &'a Document,
    ) -> Result<Self> {
        require_window()?;
        let plan = independent_plan(reference, target)?;
        let directory = path(&plan["work_dir"])?;
        validate_local_test_path(directory, backend, LocalTestPathKind::ExistingDirectory)?;
        require_absent(&directory.join(".reference-lock"))?;
        let completed = document::read(
            &directory.join("restore-candidate.json"),
            backend,
            "独立 B1 已完成外部恢复收据",
        )?;
        check_receipt(&completed.value, plan, target, backup_receipt)?;
        let (running_binding, running) =
            document::bound_file(backend, &completed.value["record"], "原 Running")?;
        if running_binding.path.parent() != Some(directory) {
            return Err("原 Running 不属于同一参考恢复目录".into());
        }
        let begin = SealedBegin::open(backend, plan, &target.value, &running)?;
        if document::binding(begin.receipt()) != completed.value["begin_receipt"] {
            return Err("已完成独立 B1 恢复未绑定原开始登记结果".into());
        }
        let work = OriginalWork::read_completed(backend, plan, begin.work())?;
        let registration = bound(
            backend,
            &completed.value["runtime_registration"],
            "原恢复运行登记",
        )?;
        check_registration(&registration, reference, target)?;
        let product = bound(backend, &target.value["product_plan_file"], "原产品计划")?;
        let manifest = bound(
            backend,
            &backup_receipt.value["result"]["manifest"],
            "原备份清单",
        )?;
        let images = &completed.value["result"]["resource_images"];
        let before = read_image(backend, directory, "before", &images["before"], target)?;
        let after = read_image(backend, directory, "after", &images["after"], target)?;
        let begin_images = begin.hold_images(backend)?;
        let files = Inputs::open(
            [
                reference,
                target,
                backup_receipt,
                &completed,
                &running,
                &registration,
                &product,
                &manifest,
                &before,
                &after,
            ]
            .into_iter()
            .map(|file| file.path.clone()),
        )?;
        let proof = Self {
            reference,
            target,
            backup: backup_receipt,
            plan,
            work,
            completed,
            running,
            registration,
            product,
            manifest,
            begin,
            begin_images,
            before,
            after,
            files,
        };
        proof.assert_unchanged()?;
        Ok(proof)
    }

    pub(in crate::recovery) fn check_work(&self, selected: &Path, plan: &Value) -> Result<()> {
        if plan != self.plan {
            return Err("独立 B1 当前投影与已完成恢复绑定的参考计划不同".into());
        }
        self.work.check_completed(selected, plan)
    }

    pub(in crate::recovery) fn verify(
        &self,
        backend: &Path,
        projected_plan: &Value,
        product_plan_hash: &str,
        target_request: &Value,
    ) -> Result<()> {
        require_window()?;
        let directory = path(&projected_plan["work_dir"])?;
        self.check_work(directory, projected_plan)?;
        self.assert_unchanged()?;
        let root = path(&self.backup.value["result"]["backup_root"])?;
        if self.manifest.path != root.join("manifest.json")
            || self.product.value != self.target.value["product_plan"]
        {
            return Err("已完成独立 B1 恢复的正式备份或产品计划不同".into());
        }
        check_running(
            &self.running,
            &self.product,
            &self.manifest,
            &self.completed,
            product_plan_hash,
        )?;
        let inputs = BeginInputs {
            root,
            reference: self.reference,
            plan: projected_plan,
            backup_receipt: self.backup,
            target: self.target,
            manifest: &self.manifest,
            registration: &self.registration,
            product_plan: &self.product,
        };
        let begin_images = self.begin.verify_images(backend, &inputs, &self.running)?;
        if *begin_images.plan_hash() != self.running.value["plan_hash"] {
            return Err("原 Running 的产品计划摘要与开始登记前像不同".into());
        }
        let initialized = bound(
            backend,
            &self.target.value["fresh_target"]["initialized"],
            "原目标初始化",
        )?;
        let (before, after) = expected_images::expected_images(
            projected_plan,
            &self.manifest.value,
            &initialized.value,
        )?;
        let initialized_directory = initialized.path.parent().ok_or("原目标初始化缺少目录")?;
        check_image(
            backend,
            &self.before,
            &before,
            initialized_directory,
            target_request,
            None,
        )?;
        check_image(
            backend,
            &self.after,
            &after,
            initialized_directory,
            target_request,
            Some(ObjectPlan {
                manifest: &self.manifest.value,
                reference: projected_plan,
                generation: &initialized.value["generation"],
            }),
        )?;
        initialized.assert_unchanged()?;
        begin_images.check()?;
        self.assert_unchanged()
    }

    pub(in crate::recovery) fn running(&self) -> &Document {
        &self.running
    }

    pub(in crate::recovery) fn manifest(&self) -> &Document {
        &self.manifest
    }

    pub(in crate::recovery) fn assert_unchanged(&self) -> Result<()> {
        let directory = path(&self.plan["work_dir"])?;
        self.check_work(directory, self.plan)?;
        for phase in ["before", "after"] {
            require_absent(&directory.join(format!("restore-candidate-{phase}/failure.json")))?;
        }
        require_absent(&directory.join("restore-candidate-failure-after"))?;
        self.files.check()?;
        for file in [
            self.reference,
            self.target,
            self.backup,
            &self.completed,
            &self.running,
            &self.registration,
            &self.product,
            &self.manifest,
            &self.before,
            &self.after,
        ] {
            file.assert_unchanged()?;
        }
        self.begin.check()?;
        self.begin_images.check()?;
        self.check_work(directory, self.plan)
    }
}

fn check_receipt(value: &Value, plan: &Value, target: &Document, backup: &Document) -> Result<()> {
    let mut fields = vec![
        "command",
        "plan_sha256",
        "started_at",
        "status",
        "completed_at",
        "result",
    ];
    fields.extend(BINDINGS);
    document::exact(value, &fields, "独立 B1 外部恢复阶段收据")?;
    let result = &value["result"];
    let mut fields = vec!["restore_id", "status", "runtime", "resource_images", "next"];
    fields.extend(BINDINGS);
    document::exact(result, &fields, "独立 B1 外部恢复阶段结果")?;
    document::exact(
        &result["resource_images"],
        &["before", "after"],
        "原恢复前后像",
    )?;
    document::exact(
        &result["runtime"],
        &["registration", "target_plan"],
        "恢复停机检查点",
    )?;
    for field in &BINDINGS[1..] {
        document::binding_shape(&value[*field], "原恢复阶段输入")?;
    }
    if value["command"] != "restore"
        || value["status"] != "completed"
        || value["target_side"] != "candidate"
        || value["plan_sha256"] != native::digest::plan_hash(plan)?
        || value["target_plan"] != document::binding(target)
        || value["backup_receipt"] != document::binding(backup)
        || target.value["backup_receipt"] != value["backup_receipt"]
        || result["status"] != "external_copy_completed"
        || result["runtime"]["registration"] != value["runtime_registration"]
        || result["runtime"]["target_plan"] != value["target_plan"]
        || BINDINGS
            .into_iter()
            .any(|field| value[field] != result[field])
        || domain::timestamp(&value["completed_at"], "外部恢复完成时间")?
            < domain::timestamp(&value["started_at"], "外部恢复开始时间")?
    {
        return Err("独立 B1 外部恢复收据未成功收尾或原计划、目标和备份绑定不同".into());
    }
    Ok(())
}

fn check_registration(
    registration: &Document,
    reference: &Document,
    target: &Document,
) -> Result<()> {
    let value = &registration.value;
    document::exact(
        value,
        &[
            "format_version",
            "kind",
            "reference_plan",
            "target_plan",
            "observation",
            "runtime_environment",
            "remote_writes",
        ],
        "原恢复运行登记",
    )?;
    let observation = &value["observation"];
    document::exact(
        observation,
        &["runtime", "ports_idle"],
        "原运行登记停止观察",
    )?;
    document::exact(
        &observation["runtime"],
        &["exists", "entries", "anchor"],
        "原运行目录观察",
    )?;
    let anchor = &observation["runtime"]["anchor"];
    document::exact(
        anchor,
        &["path", "device", "inode", "created_ns"],
        "原运行目录身份",
    )?;
    document::binding_shape(&value["runtime_environment"], "原运行私有环境绑定")?;
    if value["format_version"] != 2
        || value["kind"] != "restore-runtime-registration"
        || value["reference_plan"] != document::binding(reference)
        || value["target_plan"] != document::binding(target)
        || value["remote_writes"] != 0
        || !observation["runtime"]["exists"].is_boolean()
        || observation["runtime"]["entries"] != json!([])
        || observation["ports_idle"] != json!(["api", "worker", "frontend"])
        || !path(&anchor["path"])?.is_absolute()
        || ["device", "inode", "created_ns"]
            .iter()
            .any(|field| !anchor[*field].is_i64() && !anchor[*field].is_u64())
    {
        return Err("已完成独立 B1 恢复的原运行登记或停止证明不同".into());
    }
    Ok(())
}

fn check_running(
    running: &Document,
    product: &Document,
    manifest: &Document,
    completed: &Document,
    product_plan_hash: &str,
) -> Result<()> {
    let value = &running.value;
    document::exact(
        value,
        &[
            "plan",
            "plan_hash",
            "status",
            "started_at",
            "data_verified_at",
            "completed_at",
            "recovered_at",
            "failure",
        ],
        "原 Running 记录",
    )?;
    document::digest(&value["plan_hash"], "原 Running 产品计划摘要")?;
    let started = domain::timestamp(&value["started_at"], "原 Running 开始时间")?;
    let recovered = domain::timestamp(&value["recovered_at"], "原 Running 恢复点")?;
    let captured = domain::timestamp(&manifest.value["captured_at"], "备份恢复点")?;
    let copy_started = domain::timestamp(&completed.value["started_at"], "外部恢复开始时间")?;
    if value["status"] != "running"
        || ["data_verified_at", "completed_at", "failure"]
            .iter()
            .any(|field| !value[*field].is_null())
        || value["plan"] != product.value
        || value["plan_hash"] != product_plan_hash
        || product.value["backup_id"] != manifest.value["id"]
        || product.value["id"] != completed.value["result"]["restore_id"]
        || recovered != captured
        || recovered > started
        || started > copy_started
    {
        return Err("已完成独立 B1 恢复的开始记录、产品计划或恢复点不同".into());
    }
    Ok(())
}

fn bound(backend: &Path, descriptor: &Value, label: &str) -> Result<Document> {
    Ok(document::bound_file(backend, descriptor, label)?.1)
}

fn require_absent(path: &Path) -> Result<()> {
    reads::optional(path)?;
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err("独立 B1 已完成恢复证据存在写入锁或失败现场".into()),
    }
}

fn require_window() -> Result<()> {
    if !reads::is_active() {
        return Err("独立 B1 已完成恢复证据必须在静态读取窗口内核验".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_independent_restore_rejects_receipt_afterimage_and_registration_drift()
    -> Result<()> {
        let root = crate::workspace::root_dir();
        let backup = sample(&root, "backup.json", json!({"fixture":"backup"}))?;
        let target = sample(
            &root,
            "target.json",
            json!({"backup_receipt":document::binding(&backup)}),
        )?;
        let registration = sample(
            &root,
            "registration.json",
            json!({"fixture":"registration"}),
        )?;
        let plan = json!({"id":"backup", "work_dir":root.join(".local-tests/completed")});
        let before = sample(&root, "before-image.json", json!({"fixture":"before"}))?;
        let after = sample(&root, "after-image.json", json!({"fixture":"after"}))?;
        let mut receipt = json!({
            "command":"restore", "plan_sha256":native::digest::plan_hash(&plan)?,
            "started_at":"2026-11-24T00:30:00Z", "completed_at":"2026-11-24T00:45:00Z",
            "status":"completed", "target_side":"candidate", "target_plan":document::binding(&target),
            "backup_receipt":document::binding(&backup), "record":document::binding(&target),
            "begin_receipt":document::binding(&backup), "runtime_registration":document::binding(&registration),
        });
        let mut result = json!({
            "restore_id":"drill", "status":"external_copy_completed", "next":"verify-data",
            "runtime":{"registration":document::binding(&registration),"target_plan":document::binding(&target)},
            "resource_images":{"before":document::binding(&before),"after":document::binding(&after)},
        });
        for field in BINDINGS {
            result[field] = receipt[field].clone();
        }
        receipt["result"] = result;
        check_receipt(&receipt, &plan, &target, &backup)?;
        for (pointer, changed) in [
            ("/status", json!("running")),
            ("/result/status", json!("failed")),
            ("/completed_at", json!("2026-11-24T00:29:59Z")),
            ("/plan_sha256", json!("a".repeat(64))),
            ("/result/record", document::binding(&backup)),
            ("/result/runtime/registration", document::binding(&target)),
            (
                "/result/resource_images",
                json!({"before":document::binding(&before)}),
            ),
        ] {
            let mut other = receipt.clone();
            *other.pointer_mut(pointer).unwrap() = changed;
            assert!(
                check_receipt(&other, &plan, &target, &backup).is_err(),
                "{pointer}"
            );
        }
        Ok(())
    }

    fn sample(root: &Path, name: &str, value: Value) -> Result<Document> {
        Ok(Document {
            path: root.join(".local-tests").join(name),
            raw: document::canonical_bytes(&value)?,
            value,
        })
    }
}

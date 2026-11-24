//! 独立 B1 写入阶段只接续同一正式开始登记及其原工作目录。
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::super::super::local::OwnedWork;
use super::super::inputs::{self, RegisteredSelection};
use super::{BeginInputs, Document, Inputs, Result, document, independent, path, proof, receipt};
use crate::{
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::{
        clone::native::copy_transfer::IndependentTargetBinding,
        inputs::reference,
        monitoring::execution::ProtectedInputs,
        runtime::dataset_authority::reads::{self, Window},
    },
};

pub(in super::super) struct PendingApply {
    selection: RegisteredSelection,
    record: Document,
    record_guard: ProtectedInputs,
    begin: SealedBegin,
}

impl PendingApply {
    pub(in super::super) fn read(
        backend: &Path,
        request: &Value,
        selection: RegisteredSelection,
    ) -> Result<Self> {
        let plan = registered_plan(&selection)?;
        let work = path(&plan["work_dir"])?;
        validate_local_test_path(work, backend, LocalTestPathKind::ExistingDirectory)?;
        let (record, record_guard) = inputs::running_record(backend, request)?;
        if record.value["status"] != "running" {
            return Err("独立 B1 恢复须消费原 Running 记录".into());
        }
        let begin = SealedBegin::open(backend, plan, &selection.selected.target.value, &record)?;
        check_reference_work(work, begin.work())?;
        let bindings = &begin.intent.value["bindings"];
        for (field, file) in [
            ("reference_plan", &selection.selected.reference),
            ("target_plan", &selection.selected.target),
            ("runtime_registration", &selection.registration),
        ] {
            if bindings[field] != document::binding(file) {
                return Err("独立 B1 开始登记意图没有绑定原参考计划、目标计划和运行登记".into());
            }
        }
        let pending = Self {
            selection,
            record,
            record_guard,
            begin,
        };
        pending.check()?;
        Ok(pending)
    }

    pub(in super::super) fn claim(&self, backend: &Path) -> Result<OwnedWork> {
        self.check()?;
        let owner = OwnedWork::reenter(
            backend,
            registered_plan(&self.selection)?,
            self.begin.work(),
        )?;
        self.check()?;
        Ok(owner)
    }

    pub(in super::super) fn verify(
        self,
        backend: &Path,
        request: &Value,
        claim: &OwnedWork,
    ) -> Result<ApplySnapshot> {
        let window = Window::direct()?;
        let result = self.verify_owned(backend, request, claim);
        window.finish(result)
    }

    fn verify_owned(
        self,
        backend: &Path,
        request: &Value,
        claim: &OwnedWork,
    ) -> Result<ApplySnapshot> {
        self.check()?;
        let owner = |selected: &Path, plan: &Value| claim.check(selected, plan);
        let snapshot =
            independent::Snapshot::from_plan(backend, request, self.selection, Some(&owner))?;
        let inputs = snapshot.inputs();
        if receipt::directory(&inputs)? != self.begin.directory
            || claim.binding(backend, inputs.plan)? != *self.begin.work()
        {
            return Err("独立 B1 恢复未取得原开始登记的目录及归属".into());
        }
        proof::record(&json!({
            "plan":inputs.plan, "manifest":inputs.manifest.value, "record":self.record.value,
            "product_plan":inputs.product_plan.value,
        }))?;
        let images = self.begin.verify_images(backend, &inputs, &self.record)?;
        let verified = ApplySnapshot {
            snapshot,
            record: self.record,
            record_guard: self.record_guard,
            begin: self.begin,
            images,
        };
        verified.assert_unchanged()?;
        owner(path(&verified.plan()["work_dir"])?, verified.plan())?;
        Ok(verified)
    }

    fn check(&self) -> Result<()> {
        self.selection.registration_guard.assert_unchanged()?;
        self.selection.selected.guard.assert_unchanged()?;
        self.selection.registration.assert_unchanged()?;
        self.selection.selected.reference.assert_unchanged()?;
        self.selection.selected.target.assert_unchanged()?;
        self.record_guard.assert_unchanged()?;
        self.record.assert_unchanged()?;
        self.begin.check()
    }
}

pub(in super::super) struct ApplySnapshot {
    snapshot: independent::Snapshot,
    record: Document,
    record_guard: ProtectedInputs,
    begin: SealedBegin,
    images: HeldBeginImages,
}

impl ApplySnapshot {
    pub(in super::super) fn root(&self) -> &Path {
        self.snapshot.inputs().root
    }

    pub(in super::super) fn plan(&self) -> &Value {
        self.snapshot.inputs().plan
    }

    pub(in super::super) fn target(&self) -> &Document {
        self.snapshot.inputs().target
    }

    pub(in super::super) fn manifest(&self) -> &Document {
        self.snapshot.inputs().manifest
    }

    pub(in super::super) fn record(&self) -> &Document {
        &self.record
    }

    pub(in super::super) fn registration(&self) -> &Document {
        self.snapshot.inputs().registration
    }

    pub(in super::super) fn binding(&self) -> &IndependentTargetBinding {
        &self.snapshot.binding
    }

    pub(in super::super) fn build_bridges(&self) -> &[Value] {
        &self.snapshot.build_bridges
    }

    pub(in super::super) fn assert_unchanged(&self) -> Result<()> {
        self.snapshot.check()?;
        self.check_begin()
    }

    fn check_begin(&self) -> Result<()> {
        self.record_guard.assert_unchanged()?;
        self.record.assert_unchanged()?;
        self.begin.check()?;
        self.images.check()
    }

    pub(in super::super) fn bindings(&self) -> Value {
        let inputs = self.snapshot.inputs();
        json!({
            "target_side": inputs.target.value["target_side"],
            "target_plan": document::binding(inputs.target),
            "backup_receipt": document::binding(inputs.backup_receipt),
            "record": document::binding(self.record()),
            "begin_receipt": document::binding(&self.begin.result),
            "runtime_registration": document::binding(inputs.registration),
        })
    }

    pub(in super::super) fn check_runtime(&self, checkpoint: &Value) -> Result<()> {
        self.snapshot.check_runtime(checkpoint)?;
        self.check_begin()
    }

    pub(in super::super) fn check_final(
        &self,
        backend: &Path,
        request: &Value,
        claim: &OwnedWork,
    ) -> Result<()> {
        self.assert_unchanged()?;
        let selection = RegisteredSelection::read(backend, request)?;
        let current =
            PendingApply::read(backend, request, selection)?.verify(backend, request, claim)?;
        if current.bindings() != self.bindings() {
            return Err("独立 B1 恢复阶段首尾的开始登记或原输入绑定不同".into());
        }
        self.assert_unchanged()
    }
}

pub(in super::super) struct SealedBegin {
    directory: PathBuf,
    result: Document,
    intent: Document,
    command: Document,
    directory_guard: Inputs,
    files_guard: Inputs,
}

impl SealedBegin {
    pub(in super::super) fn open(
        backend: &Path,
        plan: &Value,
        target: &Value,
        record: &Document,
    ) -> Result<Self> {
        let directory = receipt::directory_for(plan, target)?;
        independent::verify_output_name(path(&plan["work_dir"])?, &directory, &record.path)?;
        Self::read(backend, directory, record)
    }

    pub(in super::super) fn open_recorded(
        backend: &Path,
        plan: &Value,
        target: &Value,
    ) -> Result<(Self, Document)> {
        let directory = receipt::directory_for(plan, target)?;
        let (directory_guard, result) = Self::read_result(backend, &directory)?;
        if result.value["format_version"] != 1
            || result.value["kind"] != "reference-restore-begin-result"
            || result.value["status"] != "completed"
        {
            return Err("独立 B1 原开始登记尚未成功完成".into());
        }
        let binding = document::binding_shape(&result.value["record"], "原 Running 记录")?;
        independent::verify_output_name(path(&plan["work_dir"])?, &directory, &binding.path)?;
        let record = receipt::bound(backend, &binding.path, &result.value["record"])?;
        let begin = Self::verify_result(backend, directory, directory_guard, result, &record)?;
        Ok((begin, record))
    }

    fn read(backend: &Path, directory: PathBuf, record: &Document) -> Result<Self> {
        let (directory_guard, result) = Self::read_result(backend, &directory)?;
        Self::verify_result(backend, directory, directory_guard, result, record)
    }

    fn read_result(backend: &Path, directory: &Path) -> Result<(Inputs, Document)> {
        validate_local_test_path(directory, backend, LocalTestPathKind::ExistingDirectory)?;
        let directory_guard = Inputs::directory(directory)?;
        require_no_failures(directory)?;
        let result = document::read(
            &directory.join("result.json"),
            backend,
            "独立 B1 开始登记结果",
        )?;
        Ok((directory_guard, result))
    }

    fn verify_result(
        backend: &Path,
        directory: PathBuf,
        directory_guard: Inputs,
        result: Document,
        record: &Document,
    ) -> Result<Self> {
        receipt::check_result(&result.value, record, true)?;
        let intent = receipt::bound(
            backend,
            &directory.join("intent.json"),
            &result.value["intent"],
        )?;
        receipt::check_intent(&intent.value, record, Some(&result.value["reference_work"]))?;
        let command = receipt::bound(
            backend,
            &directory.join("command.json"),
            &result.value["command"],
        )?;
        receipt::check_command(&command.value)?;
        let files_guard = Inputs::open([
            result.path.clone(),
            intent.path.clone(),
            command.path.clone(),
        ])?;
        let begin = Self {
            directory,
            result,
            intent,
            command,
            directory_guard,
            files_guard,
        };
        begin.check()?;
        Ok(begin)
    }

    pub(in super::super) fn receipt(&self) -> &Document {
        &self.result
    }

    pub(in super::super) fn work(&self) -> &Value {
        &self.result.value["reference_work"]
    }

    pub(in super::super) fn verify_images(
        &self,
        backend: &Path,
        inputs: &BeginInputs<'_>,
        record: &Document,
    ) -> Result<HeldBeginImages> {
        let images = receipt::verify_intent(
            backend,
            &self.directory,
            inputs,
            record,
            &self.intent,
            Some(self.work()),
        )?;
        HeldBeginImages::open(images)
    }

    pub(in super::super) fn hold_images(&self, backend: &Path) -> Result<HeldBeginImages> {
        HeldBeginImages::open(receipt::Images {
            preimage: receipt::bound(
                backend,
                &self.directory.join("preimage.json"),
                &self.intent.value["preimage"],
            )?,
            resource: receipt::bound(
                backend,
                &self.directory.join("resource-image.json"),
                &self.intent.value["resource_image"],
            )?,
        })
    }

    pub(in super::super) fn check(&self) -> Result<()> {
        self.directory_guard.check()?;
        require_no_failures(&self.directory)?;
        self.files_guard.check()?;
        self.result.assert_unchanged()?;
        self.intent.assert_unchanged()?;
        self.command.assert_unchanged()?;
        self.directory_guard.check()
    }
}

pub(in super::super) struct HeldBeginImages {
    images: receipt::Images,
    held: Inputs,
}

impl HeldBeginImages {
    fn open(images: receipt::Images) -> Result<Self> {
        let held = Inputs::open([images.preimage.path.clone(), images.resource.path.clone()])?;
        let result = Self { images, held };
        result.check()?;
        Ok(result)
    }

    pub(in super::super) fn check(&self) -> Result<()> {
        self.held.check()?;
        self.images.preimage.assert_unchanged()?;
        self.images.resource.assert_unchanged()
    }

    pub(in super::super) fn plan_hash(&self) -> &Value {
        &self.images.preimage.value["plan_hash"]
    }
}

fn registered_plan(selection: &RegisteredSelection) -> Result<&Value> {
    independent_plan(&selection.selected.reference, &selection.selected.target)
}

pub(in super::super) fn independent_plan<'a>(
    reference: &'a Document,
    target: &Document,
) -> Result<&'a Value> {
    let envelope = &reference.value;
    document::exact(
        envelope,
        &[
            "format_version",
            "kind",
            "reference_plan",
            "arm_input",
            "source_export",
        ],
        "原独立 B1 参考计划封装",
    )?;
    let plan = &envelope["reference_plan"];
    let target = &target.value;
    if !reference::is_independent_b1(envelope)
        || envelope["format_version"] != 1
        || plan["target_side"] != "candidate"
        || target["kind"] != "restore-reference-target-plan"
        || target["target_side"] != "candidate"
        || target["reference_plan_sha256"] != document::hash(envelope)?
    {
        return Err("独立 B1 恢复必须使用运行登记绑定的原参考计划封装和同侧目标".into());
    }
    Ok(plan)
}

fn check_reference_work(work: &Path, binding: &Value) -> Result<()> {
    document::exact(
        binding,
        &["work_dir", "directory_identity", "owner", "owner_identity"],
        "原参考目录归属",
    )?;
    let owner = document::binding_shape(&binding["owner"], "原参考目录归属文件")?;
    if path(&binding["work_dir"])? != work || owner.path != work.join("reference-owner.json") {
        return Err("独立 B1 开始登记结果绑定了另一工作目录或归属文件".into());
    }
    for field in ["directory_identity", "owner_identity"] {
        let identity = &binding[field];
        document::exact(identity, &["device", "inode"], "原参考目录内核身份")?;
        document::integer(&identity["device"], "原参考目录设备")?;
        document::integer(&identity["inode"], "原参考目录文件标识")?;
    }
    Ok(())
}

fn require_no_failures(directory: &Path) -> Result<()> {
    for name in [
        "failure.json",
        "preimage-failure.json",
        "copy-registration.json",
    ] {
        let file = directory.join(name);
        reads::optional(&file)?;
        match fs::symlink_metadata(&file) {
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
            Ok(_) => return Err("独立 B1 开始登记有失败或副本证据，必须保留并核对".into()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_begin_receipt_rejects_substituted_running_and_work_binding() -> Result<()> {
        let (_temporary, backend, directory, record) = fixture()?;
        let work = directory.parent().ok_or("开始登记测试目录缺少父目录")?;
        let plan = json!({"work_dir":work,"target_side":"candidate"});
        let target = json!({"target_side":"candidate"});
        let (recorded, selected) = SealedBegin::open_recorded(&backend, &plan, &target)?;
        assert_eq!(document::binding(&selected), document::binding(&record));
        recorded.check()?;
        drop(recorded);

        let original = SealedBegin::read(&backend, directory.clone(), &record)?;
        original.check()?;
        let mut result = original.result.value.clone();
        let mut intent = original.intent.value.clone();
        drop(original);

        let other_record = write(
            &backend,
            &record.path.with_file_name("other-running.json"),
            &record.value,
        )?;
        assert!(SealedBegin::read(&backend, directory.clone(), &other_record).is_err());
        let mut substituted = result.clone();
        substituted["record"] = document::binding(&other_record);
        write(&backend, &directory.join("result.json"), &substituted)?;
        assert!(SealedBegin::open_recorded(&backend, &plan, &target).is_err());
        intent["reference_work"]["directory_identity"]["inode"] = json!(99);
        let replaced = write(&backend, &directory.join("intent.json"), &intent)?;
        result["intent"] = document::binding(&replaced);
        write(&backend, &directory.join("result.json"), &result)?;
        assert!(SealedBegin::read(&backend, directory, &record).is_err());
        Ok(())
    }

    #[test]
    fn independent_begin_receipt_rejects_failure_and_copy_markers() -> Result<()> {
        let (_temporary, backend, directory, record) = fixture()?;
        for marker in [
            "failure.json",
            "preimage-failure.json",
            "copy-registration.json",
        ] {
            let path = directory.join(marker);
            write(&backend, &path, &json!({"fixture":true}))?;
            assert!(SealedBegin::read(&backend, directory.clone(), &record).is_err());
            fs::remove_file(path)?;
        }
        SealedBegin::read(&backend, directory, &record)?.check()
    }

    fn fixture() -> Result<(tempfile::TempDir, PathBuf, PathBuf, Document)> {
        let local = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&local)?;
        let temporary = tempfile::tempdir_in(local)?;
        let backend =
            crate::recovery::fixture_control::portable_path(&temporary.path().canonicalize()?);
        let work = backend.join(".local-tests/reference");
        let directory = work.join("restore-begin-candidate");
        fs::create_dir_all(&directory)?;
        let record = write(
            &backend,
            &work.join("running.json"),
            &json!({"status":"running"}),
        )?;
        let owner = write(
            &backend,
            &work.join("reference-owner.json"),
            &json!({"fixture":true}),
        )?;
        let work_binding = json!({
            "work_dir":work, "directory_identity":{"device":1,"inode":2},
            "owner":document::binding(&owner), "owner_identity":{"device":1,"inode":3},
        });
        check_reference_work(&work, &work_binding)?;
        let intent = write(
            &backend,
            &directory.join("intent.json"),
            &json!({
                "format_version":1, "kind":"reference-restore-begin-intent", "record_path":record.path,
                "reference_work":work_binding, "bindings":{}, "preimage":null,
                "resource_image":null, "maintenance":{},
            }),
        )?;
        let output = json!({"bytes":0,"sha256":"a".repeat(64)});
        let command = write(
            &backend,
            &directory.join("command.json"),
            &json!({
                "format_version":1, "kind":"reference-restore-begin-command", "exit_code":0,
                "success":true, "stdout":output, "stderr":output, "failure":null,
            }),
        )?;
        write(
            &backend,
            &directory.join("result.json"),
            &json!({
                "format_version":1, "kind":"reference-restore-begin-result", "status":"completed",
                "reference_work":work_binding, "intent":document::binding(&intent),
                "command":document::binding(&command), "record":document::binding(&record),
            }),
        )?;
        Ok((temporary, backend, directory, record))
    }

    fn write(backend: &Path, filename: &Path, value: &Value) -> Result<Document> {
        fs::write(filename, document::canonical_bytes(value)?)?;
        document::read(filename, backend, "开始登记测试工件")
    }
}

//! 独立 B1 参考计划只读取已收尾的原 ARM 登记。
use std::io::ErrorKind;

use super::{
    Bound, Observation, RESULT_KIND, RESULT_STATUS, RUN_KIND, arm_target::Target, common, document,
    portable_path, registered_target, require_disjoint_paths, source_history,
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::{
        clone::native::{lifecycle::identity, target_verify::guard::Inputs},
        inputs::target::{TargetRequest, validate_b1_measurement},
        runtime::dataset_authority::reads,
    },
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(in crate::recovery) struct IndependentB1Receipt<'a> {
    bound: Bound<'a>,
    recorded: Recorded,
    observed: Observation,
    storage: PathBuf,
    source_history: source_history::SourceHistory,
    pub(in crate::recovery) target_request: TargetRequest,
    pub(in crate::recovery) source_request: document::Document,
}

impl<'a> IndependentB1Receipt<'a> {
    pub(in crate::recovery) fn read(backend: &'a Path, result: &Path) -> Result<Self> {
        let recorded = Recorded::read(backend, result)?;
        let request =
            document::binding_shape(&recorded.result.value["request"], "原独立 ARM 请求")?;
        let bound = Bound::read(backend, &request.path)?;
        reads::optional(&bound.copy)?;
        validate_history(&recorded, &bound)?;
        let source_history = source_history::verify(&bound, &recorded.intent.value, &recorded.run)?;
        let observed = bound.observe(false)?;
        let intent = &recorded.intent.value;
        let expected = bound.intent(
            &recorded.manifest,
            &intent["controller"],
            &observed,
            &intent["source_state"],
            &source_history,
        );
        if *intent != expected {
            return Err("独立 B1 ARM 意图与当前源历史、目标初始化或存储账本不同".into());
        }
        let storage = registered_target(
            backend,
            &observed.target.registration,
            &observed.target.generation,
        )?;
        bound.check_paths(&recorded.run, &storage, &observed.review)?;
        let registration =
            common::read_bound(backend, &observed.target.registration, "独立 B1 目标登记")?;
        let (_, target_request) =
            document::bound_file(backend, &registration["request"], "独立 B1 目标请求")?;
        let target_request = validate_b1_measurement(backend, target_request, None)?;
        let (_, source_request) = document::bound_file(
            backend,
            &bound.source.source["source_request"],
            "共享导出原来源请求",
        )?;
        if source_request.value != bound.source.source["request"] {
            return Err("独立 B1 ARM 的原来源请求与共享导出历史不同".into());
        }
        let receipt = Self {
            bound,
            recorded,
            observed,
            storage,
            source_history,
            target_request,
            source_request,
        };
        receipt.check()?;
        Ok(receipt)
    }

    pub(in crate::recovery) fn check(&self) -> Result<()> {
        let backend = self.bound.backend;
        self.recorded.check(backend)?;
        self.bound.check()?;
        self.source_history.check(backend)?;
        let target = &self.observed.target;
        for (descriptor, label) in [
            (&target.registration, "独立 B1 目标登记"),
            (&target.initialized, "独立 B1 目标初始化"),
            (&target.files, "独立 B1 初始化文件"),
            (&target.environment, "独立 B1 目标环境"),
            (&target.state, "独立 B1 目标存储账本"),
        ] {
            common::bound_file(backend, descriptor, label)?;
        }
        if registered_target(backend, &target.registration, &target.generation)? != self.storage {
            return Err("独立 B1 目标原存储定位变化".into());
        }
        self.target_request.document.assert_unchanged()?;
        self.source_request.assert_unchanged()
    }

    pub(in crate::recovery) fn require_disjoint(&self, path: &Path) -> Result<()> {
        require_disjoint_paths(path, &self.recorded.run)?;
        self.bound
            .check_paths(path, &self.storage, &self.observed.review)
    }

    pub(in crate::recovery) fn result_binding(&self) -> Value {
        document::binding(&self.recorded.result)
    }

    pub(in crate::recovery) fn source_export(&self) -> &Value {
        &self.bound.source.identity
    }

    pub(in crate::recovery) fn initialized(&self) -> Result<(&Value, document::Document)> {
        let binding = &self.observed.target.initialized;
        let (_, initialized) =
            document::bound_file(self.bound.backend, binding, "独立 B1 目标初始化")?;
        Ok((binding, initialized))
    }

    pub(in crate::recovery) fn observed_target(&self) -> &Target {
        &self.observed.target
    }

    pub(in crate::recovery) fn build_bridges(&self) -> Result<&[Value]> {
        crate::recovery::clone::native::json::array(
            &self.bound.request.value["build_bridges"],
            "独立 B1 原构建桥接",
        )
    }

    pub(in crate::recovery) fn copy_directory(&self) -> &Path {
        &self.bound.copy
    }
}

struct Recorded {
    run: PathBuf,
    result: document::Document,
    manifest: document::Document,
    intent: document::Document,
    guard: Inputs,
}

impl Recorded {
    fn read(backend: &Path, selected: &Path) -> Result<Self> {
        validate_local_test_path(selected, backend, LocalTestPathKind::ExistingFile)?;
        let selected = portable_path(&selected.canonicalize()?);
        let run = selected
            .parent()
            .ok_or("独立 B1 ARM 缺少登记目录")?
            .to_owned();
        if selected != run.join("result.json") {
            return Err("独立 B1 ARM 必须选择原登记的 result.json".into());
        }
        require_finished(&run)?;
        let manifest_path = run.join("manifest.json");
        let intent_path = run.join("intent.json");
        let guard = Inputs::open([selected.clone(), manifest_path.clone(), intent_path.clone()])?;
        let result = document::read(&selected, backend, "独立 B1 ARM 结果")?;
        validate_result(&result.value)?;
        let manifest = read_exact_file(
            backend,
            &result.value["registration"],
            &manifest_path,
            "独立 B1 ARM 清单",
        )?;
        let intent = read_exact_file(
            backend,
            &result.value["intent"],
            &intent_path,
            "独立 B1 ARM 意图",
        )?;
        let recorded = Self {
            run,
            result,
            manifest,
            intent,
            guard,
        };
        recorded.check(backend)?;
        Ok(recorded)
    }

    fn check(&self, backend: &Path) -> Result<()> {
        require_finished(&self.run)?;
        if common::directory_identity(backend, &self.manifest.value["directory"])? != self.run {
            return Err("独立 B1 ARM 清单不属于同一物理登记目录".into());
        }
        self.guard.check()?;
        self.result.assert_unchanged()?;
        self.manifest.assert_unchanged()?;
        self.intent.assert_unchanged()?;
        require_finished(&self.run)
    }
}

fn validate_result(value: &Value) -> Result<()> {
    document::exact(
        value,
        &[
            "format_version",
            "kind",
            "status",
            "registration",
            "intent",
            "request",
            "target_side",
            "remote_writes",
            "restore_qualified",
        ],
        "独立 B1 ARM 结果",
    )?;
    if value["format_version"] != 1
        || value["kind"] != RESULT_KIND
        || value["status"] != RESULT_STATUS
        || value["target_side"] != "candidate"
        || value["remote_writes"] != 0
        || value["restore_qualified"] != false
    {
        return Err("参考计划只接受成功收尾且未授予恢复资格的独立 B1 ARM 结果".into());
    }
    Ok(())
}

fn validate_history(recorded: &Recorded, bound: &Bound<'_>) -> Result<()> {
    let manifest = &recorded.manifest.value;
    let intent = &recorded.intent.value;
    let request = document::binding(&bound.request);
    if *manifest
        != json!({
            "format_version":1, "kind":RUN_KIND, "directory":manifest["directory"],
            "id":bound.request.value["id"], "request":request,
            "request_canonical_sha256":super::digest::plan_hash(&bound.request.value)?,
            "copy_directory":bound.request.value["copy_directory"],
        })
        || recorded.result.value["request"] != request
        || intent["request"] != request
        || intent["registration"] != document::binding(&recorded.manifest)
        || intent["controller"]["manifest_sha256"] != recorded.manifest.sha256()
    {
        return Err("独立 B1 ARM 请求、原运行清单、控制器或复制路径绑定不同".into());
    }
    identity::validate(&intent["controller"], &recorded.run)
}

fn read_exact_file(
    backend: &Path,
    descriptor: &Value,
    path: &Path,
    label: &str,
) -> Result<document::Document> {
    if document::binding_shape(descriptor, label)?.path != path {
        return Err(format!("{label}必须位于同一原登记目录的固定路径").into());
    }
    document::bound_file(backend, descriptor, label).map(|(_, document)| document)
}

fn require_finished(run: &Path) -> Result<()> {
    for name in ["failure.json", "run.lock"] {
        match fs::symlink_metadata(run.join(name)) {
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
            Ok(_) => return Err(format!("独立 B1 ARM 登记仍有 {name}，保留原现场").into()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_b1_reference_reads_only_the_original_completed_receipt() -> Result<()> {
        let backend = crate::workspace::root_dir();
        let fixtures = backend.join(".local-tests");
        fs::create_dir_all(&fixtures)?;
        let fixture = tempfile::tempdir_in(&fixtures)?;
        let run = portable_path(&fixture.path().canonicalize()?);
        let (device, inode) = common::device_inode_for(&run, &fs::symlink_metadata(&run)?)?;
        let manifest = run.join("manifest.json");
        let intent = run.join("intent.json");
        let result = run.join("result.json");
        super::super::native::write_new(
            &manifest,
            &json!({"directory":{"path":run,"device":device,"inode":inode}}),
        )?;
        let original_intent = json!({"source_export":"original","target_storage_state":"original"});
        super::super::native::write_new(&intent, &original_intent)?;
        let registered = json!({
            "format_version":1, "kind":RESULT_KIND, "status":RESULT_STATUS,
            "registration":common::binding(&manifest)?, "intent":common::binding(&intent)?,
            "request":null, "target_side":"candidate", "remote_writes":0, "restore_qualified":false,
        });
        super::super::native::write_new(&result, &registered)?;
        Recorded::read(&backend, &result)?;
        fs::create_dir(run.join("run.lock"))?;
        assert!(Recorded::read(&backend, &result).is_err());
        fs::remove_dir(run.join("run.lock"))?;
        for field in ["source_export", "target_storage_state"] {
            let mut changed = original_intent.clone();
            changed[field] = json!("changed!");
            fs::write(&intent, document::canonical_bytes(&changed)?)?;
            assert!(Recorded::read(&backend, &result).is_err(), "{field}");
        }
        fs::write(&intent, document::canonical_bytes(&original_intent)?)?;
        Recorded::read(&backend, &result)?;
        super::super::native::write_new(&run.join("failure.json"), &json!({}))?;
        assert!(Recorded::read(&backend, &result).is_err());
        Ok(())
    }

    #[test]
    fn independent_b1_reference_rejects_ordinary_and_qualified_arm_results() -> Result<()> {
        let value = json!({
            "format_version":1, "kind":RESULT_KIND, "status":RESULT_STATUS,
            "registration":null, "intent":null, "request":null,
            "target_side":"candidate", "remote_writes":0, "restore_qualified":false,
        });
        validate_result(&value)?;
        for (key, wrong) in [
            ("status", json!("seed_arm_input_published")),
            ("kind", json!("devex-clone-seed-successor-arm-input")),
            ("target_side", json!("base")),
            ("remote_writes", json!(1)),
            ("restore_qualified", json!(true)),
        ] {
            let mut changed = value.clone();
            changed[key] = wrong;
            assert!(validate_result(&changed).is_err(), "{key}");
        }
        Ok(())
    }
}

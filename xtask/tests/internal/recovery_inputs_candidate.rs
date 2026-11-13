use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

use super::{
    Base, Fixture, RestoreInputSide, SourceState, capture_target_plan, prepare_arm, prepare_target,
    product, product_options, reference, reference_options, write_value,
};
use crate::recovery::inputs::{document, target_plan};
use crate::recovery_inputs_support::{binding, hash, read_value};

impl Fixture {
    fn candidate_plans(&self) -> (PathBuf, PathBuf) {
        let base = Base {
            backend: self.backend.clone(),
            root: self.root.join("candidate-target"),
            tool: self.tool.clone(),
            defaults: self.defaults.clone(),
            work_dir: self.work_dir.with_file_name("candidate-reference"),
        };
        let receipt = read_value(&self.backup_receipt);
        let export = receipt["result"]["source_export"].clone();
        let source = SourceState {
            request: export["source_request"].clone(),
            generation: export["source_generation"].clone(),
            export,
        };
        let side = RestoreInputSide::Candidate;
        let target = prepare_target(&base, side);
        let arm = prepare_arm(&base, &target, &source, side);
        let options = reference_options(
            &arm.arm_input,
            &target.fresh_verify,
            &base.work_dir,
            side,
            None,
        );
        let reference_value = reference::build(&base.backend, &options).unwrap();
        let reference_plan = write_value(&base.root, "reference.json", &reference_value);
        let mut options = product_options(
            &reference_plan,
            &self.backup_receipt,
            &self.comparison_sources,
            &arm.arm_input,
            &target.fresh_verify,
            side,
            None,
        );
        options.id = "restore-candidate".into();
        options.fault_at = self.product_value["fault_at"].as_str().unwrap().into();
        let product_value = product::build(&base.backend, &options).unwrap();
        let product_plan = write_value(&base.root, "product.json", &product_value);
        let target_value = capture_target_plan(
            &base.backend,
            &reference_plan,
            &self.backup_receipt,
            &self.comparison_sources,
            &arm.arm_input,
            &target.fresh_verify,
            &product_plan,
        )
        .unwrap();
        (
            reference_plan,
            write_value(&base.root, "target-plan.json", &target_value),
        )
    }
}

#[test]
fn both_sides_read_back_distinct_targets_from_the_same_backup() {
    let fixture = Fixture::new();
    let backup_before = fs::read(&fixture.backup_receipt).unwrap();
    let comparison_before = fs::read(&fixture.comparison_sources).unwrap();
    let comparison = read_value(&fixture.comparison_sources);
    let (candidate_reference, candidate_target) = fixture.candidate_plans();

    let mut plans = Vec::new();
    for (reference, target, side, arm, head) in [
        (
            &fixture.reference_plan,
            &fixture.target_plan,
            "base",
            "b0",
            "b",
        ),
        (
            &candidate_reference,
            &candidate_target,
            "candidate",
            "b1",
            "c",
        ),
    ] {
        let reference = document::read(reference, &fixture.backend, "参考计划").unwrap();
        let (_, value, product) = target_plan::verify(&fixture.backend, reference, target).unwrap();
        assert_eq!(value, read_value(target));
        assert_eq!(value["target_side"], side);
        assert_eq!(value["comparison_arm"], arm);
        assert_eq!(
            value["comparison_arm_sha256"],
            hash(&comparison["arms"][arm])
        );
        assert_eq!(value["backup_receipt"], binding(&fixture.backup_receipt));
        assert_eq!(
            value["comparison_sources"],
            binding(&fixture.comparison_sources)
        );
        assert_eq!(value["product_plan_file"], binding(&product.path));
        assert_eq!(value["product_plan"], product.value);
        assert_eq!(product.value["backup_id"], "backup-case-a");
        assert_eq!(product.value["frontend_sha"], head.repeat(40));
        assert_eq!(product.value["object_prefix"], format!("{side}-a/"));
        let execution = &value["product_execution"];
        assert_eq!(execution["roots"], comparison["arms"][arm]["roots"]);
        assert_eq!(execution["builds"], comparison["arms"][arm]["builds"]);
        assert_eq!(execution["adapter"], comparison["arms"][arm]["adapter"]);
        assert_ne!(
            execution["roots"]["execution_backend"],
            value["maintenance_execution"]["root"]
        );
        plans.push(value);
    }
    let (base, candidate) = (&plans[0], &plans[1]);
    for pointer in [
        "/arm_input",
        "/fresh_target/registration",
        "/fresh_target/ownership_sha256",
        "/product_plan/databases",
        "/product_plan/api_ready_url",
        "/comparison_arm_sha256",
    ] {
        assert_ne!(
            base.pointer(pointer).unwrap(),
            candidate.pointer(pointer).unwrap(),
            "{pointer}"
        );
    }
    assert_eq!(
        base["maintenance_execution"]["root"],
        candidate["maintenance_execution"]["root"]
    );
    assert_eq!(
        base["maintenance_execution"]["binding"],
        candidate["maintenance_execution"]["binding"]
    );
    assert_eq!(fs::read(&fixture.backup_receipt).unwrap(), backup_before);
    assert_eq!(
        fs::read(&fixture.comparison_sources).unwrap(),
        comparison_before
    );
}

#[test]
fn candidate_rejects_base_target_plan_and_arm() {
    let fixture = Fixture::new();
    let (candidate_reference, candidate_target) = fixture.candidate_plans();
    let reference = document::read(&candidate_reference, &fixture.backend, "参考计划").unwrap();
    let error = target_plan::verify(&fixture.backend, reference, &fixture.target_plan)
        .err()
        .expect("B1 accepted B0 target plan")
        .to_string();
    assert!(error.contains("类型、侧别或摘要"), "{error}");

    let missing_arm = read_value(&candidate_target);
    let fresh = PathBuf::from(
        missing_arm["fresh_target"]["verify"]["path"]
            .as_str()
            .unwrap(),
    );
    let options = product_options(
        &candidate_reference,
        &fixture.backup_receipt,
        &fixture.comparison_sources,
        &fixture.arm_input,
        &fresh,
        RestoreInputSide::Candidate,
        None,
    );
    let error = product::build(&fixture.backend, &options)
        .unwrap_err()
        .to_string();
    assert!(error.contains("外层结果状态或目标侧无效"), "{error}");
}

#[test]
fn backup_and_comparison_must_keep_the_same_export_and_stop_record() {
    let fixture = Fixture::new();
    let mut receipt = read_value(&fixture.backup_receipt);
    let generation = &receipt["result"]["source_generation"];
    let replacement = same_bytes_at(&fixture.root, generation, "other-stop.json");
    receipt["result"]["source_generation"] = replacement;
    fs::write(
        &fixture.backup_receipt,
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();

    let error = product::build(&fixture.backend, &fixture.product_options(None))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("备份收据与共享导出未绑定同一停止代次"),
        "{error}"
    );

    receipt["result"]["source_generation"] =
        receipt["result"]["source_export"]["source_generation"].clone();
    fs::write(
        &fixture.backup_receipt,
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
    let mut comparison = read_value(&fixture.comparison_sources);
    let export = &mut comparison["source_export"];
    export["result"] = same_bytes_at(&fixture.root, &export["result"], "other-export-result.json");
    export.as_object_mut().unwrap().remove("identity_sha256");
    let identity = hash(export);
    export["identity_sha256"] = Value::String(identity.clone());
    for arm in ["b0", "b1"] {
        comparison["arms"][arm]["source_export_identity_sha256"] = Value::String(identity.clone());
    }
    fs::write(
        &fixture.comparison_sources,
        serde_json::to_vec(&comparison).unwrap(),
    )
    .unwrap();

    let error = product::build(&fixture.backend, &fixture.product_options(None))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("未绑定同一备份、比较来源或 fresh 目标"),
        "{error}"
    );
}

fn same_bytes_at(root: &Path, descriptor: &Value, name: &str) -> Value {
    let path = root.join(name);
    fs::copy(descriptor["path"].as_str().unwrap(), &path).unwrap();
    binding(&path)
}

#[test]
fn product_input_rejects_unknown_reference_fields_at_each_nested_boundary() {
    let fixture = Fixture::new();
    let options = fixture.product_options(None);
    let expected = product::build(&fixture.backend, &options).unwrap();
    let original = fs::read(&fixture.reference_plan).unwrap();
    for (pointer, label) in [
        ("", "参考计划"),
        ("/target", "参考计划 target 侧"),
        ("/tools/mysql", "参考工具"),
        ("/target/s3", "对象存储定义"),
        ("/target/databases/0", "参考数据库"),
    ] {
        let mut reference = fixture.reference_value.clone();
        reference
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("unexpected".into(), Value::Bool(true));
        fs::write(
            &fixture.reference_plan,
            serde_json::to_vec(&reference).unwrap(),
        )
        .unwrap();
        let error = product::build(&fixture.backend, &options)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(label) && error.contains("未知字段"),
            "{pointer}: {error}"
        );
    }
    fs::write(&fixture.reference_plan, original).unwrap();
    assert_eq!(
        product::build(&fixture.backend, &options).unwrap(),
        expected
    );
}

#[test]
fn reference_input_rejects_substituted_arm_exports_and_maintenance_root() {
    let base = super::setup();
    let side = RestoreInputSide::Base;
    let mut target = prepare_target(&base, side);
    let source = super::prepare_source(&base);
    let arm = prepare_arm(&base, &target, &source, side);
    let options = reference_options(
        &arm.arm_input,
        &target.fresh_verify,
        &base.work_dir,
        side,
        None,
    );
    let expected = reference::build(&base.backend, &options).unwrap();
    let original = read_value(&arm.arm_input);
    for field in ["source_export", "source_export_result"] {
        let mut changed = original.clone();
        changed[field] =
            same_bytes_at(&base.root, &original[field], &format!("other-{field}.json"));
        let alternate = &changed[field];
        document::bound_file(&base.backend, alternate, "替代导出文件").unwrap();
        write_value(&base.root, "arm-result.json", &changed);
        let error = reference::build(&base.backend, &options)
            .unwrap_err()
            .to_string();
        assert_eq!(
            error, "successor arm-input 外层结果未完整绑定当前共享导出",
            "{field}"
        );
    }
    write_value(&base.root, "arm-result.json", &original);
    assert_eq!(reference::build(&base.backend, &options).unwrap(), expected);

    // 重绑所有引用，使维护根不一致由正式身份检查拒绝。
    target.review["scopes"]["base"]["backend_dir"] =
        Value::String(base.root.join("other-backend").to_string_lossy().into());
    let review_path = write_value(&base.root, "review.json", &target.review);
    let mut request = read_value(&base.root.join("target-request.json"));
    request["review"] = super::review_binding(&review_path, &target.review);
    let request_path = write_value(&base.root, "target-request.json", &request);
    let mut registration = read_value(Path::new(
        target.registration_binding["path"].as_str().unwrap(),
    ));
    registration["request"] = binding(&request_path);
    let registration_path = write_value(
        &base.root,
        "target-workspace/registration.json",
        &registration,
    );
    target.registration_binding = binding(&registration_path);
    let mut prepared = read_value(&base.root.join("target-workspace/prepared-files.json"));
    prepared["registration"] = target.registration_binding.clone();
    let prepared_path = write_value(
        &base.root,
        "target-workspace/prepared-files.json",
        &prepared,
    );
    let mut files = read_value(Path::new(
        target.initialized_files_binding["path"].as_str().unwrap(),
    ));
    files["registration"] = target.registration_binding.clone();
    files["predecessor"] = binding(&prepared_path);
    let files_path = write_value(
        &base.root,
        "target-workspace/initialized-files.json",
        &files,
    );
    target.initialized_files_binding = binding(&files_path);
    prepare_arm(&base, &target, &source, side);
    let error = reference::build(&base.backend, &options)
        .unwrap_err()
        .to_string();
    assert_eq!(error, "正式恢复输入未绑定同一 arm、目标侧或维护源码");
}

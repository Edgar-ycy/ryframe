use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::cli::{
    BindingsInputOptions, ProductInputOptions, RecoveryInputsCommand, ReferenceInputOptions,
    RestoreInputPublication, RestoreInputSide,
};
use super::recovery::inputs::{
    backup, bindings, chain, document, product, publish, reference, reference_configuration, run,
    target, target_plan,
};
use super::recovery_inputs_fixture::{Fixture, product_options, reference_options};
use super::recovery_inputs_support::{binding, file_sha, hash, read_value};
use crate::clone_arm_storage::arm_storage_bindings;

impl Fixture {
    pub(crate) fn reference_options(&self, publication: Option<PathBuf>) -> ReferenceInputOptions {
        reference_options(
            &self.arm_input,
            &self.fresh_verify,
            &self.work_dir,
            RestoreInputSide::Base,
            publication.map(|output| RestoreInputPublication { output }),
        )
    }

    pub(crate) fn product_options(&self, publication: Option<PathBuf>) -> ProductInputOptions {
        product_options(
            &self.reference_plan,
            &self.backup_receipt,
            &self.comparison_sources,
            &self.arm_input,
            &self.fresh_verify,
            RestoreInputSide::Base,
            publication.map(|output| RestoreInputPublication { output }),
        )
    }

    pub(crate) fn bindings_options(&self, publication: Option<PathBuf>) -> BindingsInputOptions {
        BindingsInputOptions {
            reference_plan: self.reference_plan.clone(),
            target_plan: self.target_plan.clone(),
            backup_receipt: self.backup_receipt.clone(),
            record: self.record.clone(),
            publication: publication.map(|output| RestoreInputPublication { output }),
        }
    }
}

#[test]
fn reference_product_and_bindings_native_chains_succeed() {
    let fixture = Fixture::new();

    let reference = fixture.reference_value.clone();
    assert_eq!(reference, fixture.reference_value);
    assert_eq!(reference["target_side"], "base");
    let serialized = serde_json::to_string(&reference).unwrap();
    for secret in [
        "access_key",
        "secret_key",
        "source-access",
        "source-secret",
        "target-access",
        "target-secret",
    ] {
        assert!(!serialized.contains(secret));
    }
    for (side, directory) in [
        ("source", fixture.root.join("source-runtime").join("config")),
        ("target", fixture.root.join("config")),
    ] {
        let binding = &reference[side]["s3"]["configuration"];
        assert_eq!(binding["app_env"], "test");
        assert_eq!(
            fs::canonicalize(binding["directory"].as_str().unwrap()).unwrap(),
            fs::canonicalize(directory).unwrap()
        );
    }
    let source_request = read_json(&fixture.root.join("source-request.json"));
    let path = fixture.root.join("source-request.json");
    let before = fs::read(&path).unwrap();
    let descriptor =
        document::binding(&document::read(&path, &fixture.backend, "私有源请求").unwrap());
    let projected = reference::source_projection(&fixture.backend, &descriptor).unwrap();
    assert_eq!(projected.as_object().unwrap().len(), 2);
    assert_eq!(projected["source"], reference["source"]);
    assert_eq!(projected["tools"], reference["tools"]);
    assert_eq!(source_request["source"]["s3"].as_object().unwrap().len(), 1);
    assert_eq!(projected["source"]["s3"].as_object().unwrap().len(), 3);
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(source_request["tools"].as_object().unwrap().len(), 3);
    assert!(source_request["tools"].get("node").is_some());
    assert_eq!(reference["tools"].as_object().unwrap().len(), 2);
    assert!(reference["tools"].get("node").is_none());
    for name in ["mysql", "mysqldump"] {
        assert_eq!(reference["tools"][name], source_request["tools"][name]);
    }
    assert_eq!(
        reference["target"]["worker_ready_url"],
        "http://127.0.0.1:18081/readyz"
    );

    let product = product::build(&fixture.backend, &fixture.product_options(None)).unwrap();
    assert_eq!(product["id"], "restore-case-a");
    assert_eq!(product["backup_id"], "backup-case-a");
    assert_eq!(product["frontend_sha"], "b".repeat(40));
    assert_eq!(product["object_prefix"], "base-a/");

    let bindings = bindings::build(&fixture.backend, &fixture.bindings_options(None)).unwrap();
    assert_eq!(bindings["record"], read_json(&fixture.record));
    assert_eq!(bindings["manifest"]["id"], reference["id"]);
    assert_eq!(bindings["manifest"]["source_sha"], "a".repeat(40));

    let original = fs::read(&fixture.comparison_sources).unwrap();
    let comparison = read_value(&fixture.comparison_sources);
    for (role, diagnostic) in [
        ("backend", "路径必须位于当前后端 .local-tests"),
        ("frontend", "本侧执行根的固定 dist 路径"),
    ] {
        let mut swapped = comparison.clone();
        swapped["arms"]["b0"]["builds"][role] = comparison["arms"]["b1"]["builds"][role].clone();
        fs::write(
            &fixture.comparison_sources,
            serde_json::to_vec(&swapped).unwrap(),
        )
        .unwrap();
        let error = product::build(&fixture.backend, &fixture.product_options(None))
            .unwrap_err()
            .to_string();
        assert!(error.contains(diagnostic), "{error}");
    }
    fs::write(&fixture.comparison_sources, original).unwrap();
}

#[test]
fn reference_plan_summary_process_leaves_the_fixture_and_cold_cache_untouched() {
    use std::{collections::BTreeMap, path::PathBuf, process::Command};

    fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
        let mut snapshot = BTreeMap::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(directory).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                let relative = path.strip_prefix(root).unwrap().to_owned();
                if entry.file_type().unwrap().is_dir() {
                    pending.push(path);
                    snapshot.insert(relative, None);
                } else {
                    snapshot.insert(relative, Some(fs::read(path).unwrap()));
                }
            }
        }
        snapshot
    }

    let fixture = Fixture::new();
    let cache = fixture.root.join("cold-bytecode");
    let before = snapshot(&fixture.root);
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["check", "recovery", "plan", "--plan"])
        .arg(&fixture.reference_plan)
        .current_dir(&fixture.backend)
        .env("RYFRAME_XTASK_RECOVERY_REFERENCE", "untrusted")
        .env("RYFRAME_XTASK_RECOVERY_INPUTS", "untrusted")
        .env("PYTHONPYCACHEPREFIX", &cache)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let summary: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["id"], fixture.reference_value["id"]);
    assert_eq!(summary["plan_sha256"], hash(&fixture.reference_value));
    assert_eq!(summary["work_dir"], fixture.reference_value["work_dir"]);
    for side in ["source", "target"] {
        let databases = summary["targets"][side].as_array().unwrap();
        let expected = fixture.reference_value[side]["databases"]
            .as_array()
            .unwrap();
        assert_eq!(databases.len(), expected.len());
        for (database, expected) in databases.iter().zip(expected) {
            assert_eq!(
                *database,
                json!({"key":expected["key"],"server_uuid":expected["server_uuid"],"database":expected["database"]}),
            );
        }
    }
    assert!(!cache.exists());
    assert_eq!(snapshot(&fixture.root), before);
}

#[test]
fn target_plan_checks_each_original_document_before_reconstructing_the_chain() {
    let fixture = Fixture::new();
    let reference = document::read(&fixture.reference_plan, &fixture.backend, "参考计划").unwrap();
    let original = read_value(&fixture.target_plan);
    let verify = || target_plan::verify(&fixture.backend, reference.clone(), &fixture.target_plan);
    assert_eq!(verify().unwrap().1, original);

    let owner = fixture.work_dir.join("reference-owner.json");
    fs::write(&owner, b"{}").unwrap();
    let reason = |result: crate::Result<_>| {
        result
            .err()
            .expect("expected invalid target plan")
            .to_string()
    };
    assert!(reason(verify()).contains("ownership"));
    for (pointer, label) in [
        ("/backup_receipt", "目标计划备份收据"),
        ("/comparison_sources", "目标计划双版本来源"),
        ("/arm_input", "目标计划 arm 输入"),
        ("/product_plan_file", "目标计划产品恢复文件"),
        ("/fresh_target/verify", "目标计划目标复核"),
        ("/fresh_target/initialized", "目标计划初始化结果"),
        ("/fresh_target/registration", "目标计划初始化登记"),
        ("/fresh_target/initialized_files", "目标计划初始化文件"),
        ("/fresh_target/environment", "目标计划初始化环境"),
        ("/fresh_target/inventory", "目标计划初始化库存"),
        ("/maintenance_execution/build", "目标计划维护构建"),
        ("/product_execution/builds/backend", "后端构建收据"),
        ("/product_execution/builds/frontend", "前端构建收据"),
    ] {
        let descriptor = original.pointer(pointer).unwrap();
        let path = Path::new(descriptor["path"].as_str().unwrap());
        let bytes = fs::read(path).unwrap();
        let mut changed = bytes.clone();
        let whitespace = changed.iter_mut().find(|byte| **byte == b'\n').unwrap();
        *whitespace = b' ';
        assert_eq!(
            serde_json::from_slice::<Value>(&changed).unwrap(),
            read_value(path)
        );
        fs::write(path, changed).unwrap();
        let error = reason(verify());
        fs::write(path, bytes).unwrap();
        assert!(
            error.contains(label) && !error.contains("ownership"),
            "{pointer}: {error}"
        );
    }
    assert!(reason(verify()).contains("ownership"));
    assert_eq!(read_value(&fixture.target_plan), original);
}

#[test]
fn target_plan_rejects_substituted_execution_authorities() {
    let fixture = Fixture::new();
    let reference = document::read(&fixture.reference_plan, &fixture.backend, "参考计划").unwrap();
    let original = read_value(&fixture.target_plan);
    let comparison = read_value(&fixture.comparison_sources);
    let candidate = &comparison["arms"]["b1"];
    let other_execution = json!({
        "roots":candidate["roots"],
        "backend_product_sha":candidate["sources"]["backend"]["source"]["snapshot"]["head"],
        "backend_execution_sha":candidate["execution_sources"]["backend"]["source"]["snapshot"]["head"],
        "frontend_sha":candidate["sources"]["frontend"]["source"]["snapshot"]["head"],
        "builds":candidate["builds"],
        "adapter":candidate["adapter"],
    });
    for (pointer, value, expected) in [
        ("/format_version", json!(true), "类型、侧别或摘要"),
        ("/target_side", json!("candidate"), "类型、侧别或摘要"),
        ("/comparison_arm", json!("b1"), "类型、侧别或摘要"),
        (
            "/product_plan_sha256",
            json!("f".repeat(64)),
            "类型、侧别或摘要",
        ),
        (
            "/comparison_arm_sha256",
            json!("f".repeat(64)),
            "当前完整来源",
        ),
        (
            "/fresh_target/ownership_sha256",
            json!("f".repeat(64)),
            "当前完整来源",
        ),
        (
            "/maintenance_execution/root",
            json!(fixture.root),
            "当前完整来源",
        ),
        (
            "/maintenance_execution/binding/kind",
            json!("other"),
            "当前完整来源",
        ),
        (
            "/product_execution/backend_execution_sha",
            json!("f".repeat(40)),
            "当前完整来源",
        ),
        (
            "/product_execution/backend_product_sha",
            json!("f".repeat(40)),
            "当前完整来源",
        ),
        ("/product_execution/adapter", Value::Null, "当前完整来源"),
        (
            "/product_execution/roots/execution_backend",
            json!(fixture.backend),
            "当前完整来源",
        ),
        ("/product_execution", other_execution, "当前完整来源"),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        fs::write(&fixture.target_plan, serde_json::to_vec(&changed).unwrap()).unwrap();
        let error = target_plan::verify(&fixture.backend, reference.clone(), &fixture.target_plan)
            .err()
            .expect("substituted target plan accepted")
            .to_string();
        assert!(error.contains(expected), "{pointer}: {error}");
    }
}

#[test]
fn target_plan_rejects_unknown_fields_and_missing_bindings() {
    let fixture = Fixture::new();
    let reference = document::read(&fixture.reference_plan, &fixture.backend, "参考计划").unwrap();
    let original = read_value(&fixture.target_plan);
    for (pointer, label) in [
        ("", "目标计划"),
        ("/fresh_target", "目标计划 fresh_target"),
        ("/backup_receipt", "目标计划备份收据"),
        ("/maintenance_execution", "目标计划维护执行"),
        ("/product_execution", "目标计划产品执行"),
    ] {
        let mut changed = original.clone();
        changed
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("unexpected".into(), json!(true));
        fs::write(&fixture.target_plan, serde_json::to_vec(&changed).unwrap()).unwrap();
        let error = target_plan::verify(&fixture.backend, reference.clone(), &fixture.target_plan)
            .err()
            .expect("unknown target field accepted")
            .to_string();
        assert!(
            error.contains(label) && error.contains("未知字段"),
            "{pointer}: {error}"
        );
    }
    let mut missing = original.clone();
    missing.as_object_mut().unwrap().remove("backup_receipt");
    fs::write(&fixture.target_plan, serde_json::to_vec(&missing).unwrap()).unwrap();
    let error = target_plan::verify(&fixture.backend, reference.clone(), &fixture.target_plan)
        .err()
        .expect("missing binding accepted")
        .to_string();
    assert!(error.contains("目标计划字段缺失"), "{error}");
    fs::write(&fixture.target_plan, serde_json::to_vec(&original).unwrap()).unwrap();
    assert_eq!(
        target_plan::verify(&fixture.backend, reference, &fixture.target_plan)
            .unwrap()
            .1,
        original
    );
}

#[test]
fn arm_storage_requires_the_registered_run_prefix_and_same_effective_manifest() {
    let fixture = Fixture::new();
    let options = fixture.reference_options(None);
    let result_before = fs::read(&fixture.arm_input).unwrap();
    let mut result = read_json(&fixture.arm_input);
    let manifest_path = Path::new(result["manifest"]["path"].as_str().unwrap()).to_owned();
    let manifest_before = fs::read(&manifest_path).unwrap();
    let mut manifest = read_json(&manifest_path);
    let storage = &manifest["target_storage_run"];
    let state_path = Path::new(storage["path"].as_str().unwrap()).join("state.json");
    let (_, other) = arm_storage_bindings(&fixture.root.join("other-storage-run"));

    manifest["target_storage_run"] = other.clone();
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    result["manifest"] = binding(&manifest_path);
    fs::write(
        &fixture.arm_input,
        serde_json::to_vec_pretty(&result).unwrap(),
    )
    .unwrap();
    let mismatch = reference::build(&fixture.backend, &options).unwrap_err();
    assert_eq!(
        mismatch.to_string(),
        "successor arm-input 清单的 target_storage_run 不一致"
    );

    result["target_storage_run"] = other;
    fs::write(
        &fixture.arm_input,
        serde_json::to_vec_pretty(&result).unwrap(),
    )
    .unwrap();
    let unrelated = reference::build(&fixture.backend, &options).unwrap_err();
    assert_eq!(
        unrelated.to_string(),
        "target_storage_run 不属于 fresh 初始化登记代次"
    );

    fs::write(&manifest_path, manifest_before).unwrap();
    fs::write(&fixture.arm_input, result_before).unwrap();
    let mut state = read_json(&state_path);
    state["attempts"][0]["sources"]["fixture"] = json!(false);
    fs::write(&state_path, serde_json::to_vec_pretty(&state).unwrap()).unwrap();
    let drift = reference::build(&fixture.backend, &options).unwrap_err();
    assert_eq!(drift.to_string(), "历史阶段记录不是当前账本的精确追加前缀");
}

#[test]
fn dry_run_writes_nothing_and_publication_is_create_only() {
    let fixture = Fixture::new();
    let dry_work = fixture.root.join("work").join("dry-reference");
    let mut options = fixture.reference_options(None);
    options.work_dir = dry_work.clone();
    let command = RecoveryInputsCommand::Reference(options.clone());
    run(&command, &fixture.backend).unwrap();
    assert!(!fixture.output.exists());
    assert!(!dry_work.exists());

    options.publication = Some(RestoreInputPublication {
        output: fixture.output.clone(),
    });
    let publish = RecoveryInputsCommand::Reference(options);
    run(&publish, &fixture.backend).unwrap();
    let original = fs::read(&fixture.output).unwrap();
    let result = run(&publish, &fixture.backend);
    assert!(result.is_err());
    assert_eq!(fs::read(&fixture.output).unwrap(), original);
    let pending = fs::read_dir(&fixture.root)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_name().to_string_lossy().contains(&format!(
                ".{}",
                fixture.output.file_name().unwrap().to_string_lossy()
            ))
        })
        .collect::<Vec<_>>();
    assert!(pending.is_empty(), "create-only 冲突后应清理临时文件");
}

#[test]
fn path_escape_missing_and_unknown_structures_are_rejected() {
    let fixture = Fixture::new();
    let mut escaped = fixture.reference_options(None);
    escaped.work_dir = fixture
        .backend
        .join(".local-tests")
        .join("..")
        .join("escaped");
    assert!(reference::build(&fixture.backend, &escaped).is_err());

    let mut missing = fixture.reference_options(None);
    missing.arm_input = fixture.root.join("missing.json");
    assert!(reference::build(&fixture.backend, &missing).is_err());

    let mut unknown = read_json(&fixture.arm_input);
    unknown
        .as_object_mut()
        .unwrap()
        .insert("unknown".into(), Value::Bool(true));
    let unknown_path = fixture.root.join("arm-unknown.json");
    fs::write(&unknown_path, serde_json::to_vec_pretty(&unknown).unwrap()).unwrap();
    let mut invalid = fixture.reference_options(None);
    invalid.arm_input = unknown_path;
    assert!(reference::build(&fixture.backend, &invalid).is_err());
}

#[test]
fn invalid_identifier_and_utc_time_are_rejected() {
    let fixture = Fixture::new();
    let mut invalid_id = fixture.reference_options(None);
    invalid_id.id = "Upper-Case".into();
    assert!(reference::build(&fixture.backend, &invalid_id).is_err());

    let mut invalid_time = fixture.product_options(None);
    invalid_time.fault_at = "2026-10-18T00:00:00+08:00".into();
    assert!(product::build(&fixture.backend, &invalid_time).is_err());
}

#[test]
fn publication_requires_create_only_file_semantics() {
    let fixture = Fixture::new();
    let target = fixture.root.join("direct-output.json");
    let value = fixture.reference_value.clone();
    let mut command = fixture.reference_options(Some(target.clone()));
    command.work_dir = fixture.root.join("work").join("direct-work");
    command.publication = Some(RestoreInputPublication {
        output: target.clone(),
    });
    run(&RecoveryInputsCommand::Reference(command), &fixture.backend).unwrap();
    assert!(target.is_file());
    let racy = fixture.root.join("racy-output.json");
    fs::write(&racy, b"external writer").unwrap();
    let mut race = fixture.reference_options(Some(racy.clone()));
    race.work_dir = fixture.root.join("work").join("race-work");
    race.publication = Some(RestoreInputPublication {
        output: racy.clone(),
    });
    assert!(run(&RecoveryInputsCommand::Reference(race), &fixture.backend,).is_err());
    assert_eq!(fs::read(&racy).unwrap(), b"external writer");
    assert_eq!(value["format_version"], 1);
}

fn read_json(path: &std::path::Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn bindings_reject_a_different_explicit_backup_and_unverified_record() {
    let fixture = Fixture::new();
    let other = fixture.root.join("other-backup.json");
    fs::copy(&fixture.backup_receipt, &other).unwrap();
    let mut options = fixture.bindings_options(None);
    options.backup_receipt = other;
    assert!(bindings::build(&fixture.backend, &options).is_err());
    let mut record = read_json(&fixture.record);
    record["status"] = Value::String("started".into());
    fs::write(&fixture.record, serde_json::to_vec(&record).unwrap()).unwrap();
    assert!(bindings::build(&fixture.backend, &fixture.bindings_options(None)).is_err());
}

#[test]
fn reference_rejects_side_generation_and_runtime_overlap() {
    let fixture = Fixture::new();
    let mut options = fixture.reference_options(None);
    options.side = super::cli::RestoreInputSide::Candidate;
    assert!(reference::build(&fixture.backend, &options).is_err());
    options = fixture.reference_options(None);
    let runtime = std::path::PathBuf::from(
        fixture.reference_value["target"]["runtime_dir"]
            .as_str()
            .unwrap(),
    );
    fs::create_dir_all(&runtime).unwrap();
    options.work_dir = runtime.join("restore");
    assert!(reference::build(&fixture.backend, &options).is_err());
    let mut arm = read_json(&fixture.arm_input);
    arm["source_generation"] = serde_json::json!({"changed": true});
    fs::write(&fixture.arm_input, serde_json::to_vec(&arm).unwrap()).unwrap();
    assert!(reference::build(&fixture.backend, &fixture.reference_options(None)).is_err());
}

#[test]
fn publication_preserves_post_write_drift_and_cleans_pending_files() {
    let fixture = Fixture::new();
    let calls = std::cell::Cell::new(0);
    let value = serde_json::json!({"id": "original"});
    let rejected = publish::finish("reference", Some(&fixture.output), &value, || {
        Ok(serde_json::json!({"id": "changed before publication"}))
    });
    assert!(rejected.unwrap_err().to_string().contains("发布前"));
    assert!(!fixture.output.exists());
    let result = publish::finish("reference", Some(&fixture.output), &value, || {
        calls.set(calls.get() + 1);
        Ok(if calls.get() == 1 {
            value.clone()
        } else {
            serde_json::json!({"id": "changed"})
        })
    });
    assert!(result.unwrap_err().to_string().contains("发布后"));
    assert_eq!(read_json(&fixture.output), value);
    assert!(!fs::read_dir(&fixture.root).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".pending")
    }));
}

#[test]
fn publication_rejects_truncated_reread_and_preserves_concurrent_writer() {
    let fixture = Fixture::new();
    let value = serde_json::json!({"message": "中文", "id": 1});
    let content = document::canonical_bytes(&value).unwrap();
    fs::write(&fixture.output, &content[..content.len() - 1]).unwrap();
    assert!(document::verify_published(&fixture.output, &value, &content).is_err());
    fs::remove_file(&fixture.output).unwrap();
    let result = publish::finish("reference", Some(&fixture.output), &value, || {
        fs::write(&fixture.output, b"concurrent unrelated writer").unwrap();
        Ok(value.clone())
    });
    assert!(result.unwrap_err().to_string().contains("并发创建"));
    assert_eq!(
        fs::read(&fixture.output).unwrap(),
        b"concurrent unrelated writer"
    );
    assert!(!fs::read_dir(&fixture.root).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".pending")
    }));
}

#[test]
fn document_rejects_duplicate_keys_and_invalid_utf8() {
    let fixture = Fixture::new();
    let path = fixture.root.join("invalid.json");
    for bytes in [br#"{"side":"base","side":"candidate"}"#.as_slice(), &[0xff]] {
        fs::write(&path, bytes).unwrap();
        assert!(document::read(&path, &fixture.backend, "输入").is_err());
    }
}

#[test]
fn document_rejects_invalid_files_and_detects_same_size_changes() {
    let fixture = Fixture::new();
    let path = fixture.root.join("document.json");
    let read = || document::read(&path, &fixture.backend, "恢复文档");
    let directory_error = document::read(&fixture.root, &fixture.backend, "恢复文档")
        .unwrap_err()
        .to_string();
    assert!(directory_error.contains("输入路径必须是现有普通文件"));

    for (bytes, expected) in [
        (b"".as_slice(), "不能为空"),
        (b"[]".as_slice(), "必须是 JSON 对象"),
        (br#"{"value":NaN}"#.as_slice(), "不是严格 JSON"),
    ] {
        fs::write(&path, bytes).unwrap();
        let error = read().unwrap_err().to_string();
        assert!(error.contains(expected), "{error}");
    }
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(document::MAX_JSON_BYTES + 1)
        .unwrap();
    let error = read().unwrap_err().to_string();
    assert!(error.contains("超过 16 MiB"), "{error}");

    let original = br#"{"value":1}"#;
    fs::write(&path, original).unwrap();
    let saved = read().unwrap();
    assert_eq!(saved.value, json!({"value":1}));
    assert_eq!(saved.raw.as_slice(), original);
    assert_eq!(saved.sha256(), file_sha(&path));
    saved.assert_unchanged().unwrap();
    fs::write(&path, br#"{"value":2}"#).unwrap();
    let changed = read().unwrap();
    assert_eq!(saved.raw.len(), changed.raw.len());
    assert_eq!(changed.value, json!({"value":2}));
    let error = saved.assert_unchanged().unwrap_err().to_string();
    assert!(error.contains("被替换或修改"), "{error}");
}

#[cfg(any(unix, windows))]
#[test]
fn document_rejects_file_links_and_windows_junction_ancestors() {
    let fixture = Fixture::new();
    let directory = fixture.root.join("document-directory");
    fs::create_dir(&directory).unwrap();
    let target = directory.join("document.json");
    let raw = br#"{"value":7}"#;
    fs::write(&target, raw).unwrap();
    let read = |path: &Path| document::read(path, &fixture.backend, "恢复文档");
    assert_eq!(read(&target).unwrap().raw.as_slice(), raw);

    let link = fixture.root.join("document-link.json");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &link).expect("创建文件符号链接夹具");
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&target, &link).expect("创建文件符号链接夹具");
    let linked_bytes = fs::read(&link);
    let linked_document = read(&link);
    fs::remove_file(&link).unwrap();
    assert_eq!(linked_bytes.unwrap(), raw);
    let error = linked_document.unwrap_err().to_string();
    assert!(error.contains("路径不能经过符号链接或 junction"), "{error}");

    #[cfg(windows)]
    {
        let junction = fixture.root.join("document-junction");
        let output = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&directory)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "创建目录联接夹具失败：{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let aliased = junction.join("document.json");
        let aliased_bytes = fs::read(&aliased);
        let aliased_document = read(&aliased);
        fs::remove_dir(&junction).unwrap();
        assert_eq!(aliased_bytes.unwrap(), raw);
        let error = aliased_document.unwrap_err().to_string();
        assert!(error.contains("路径不能经过符号链接或 junction"), "{error}");
    }

    assert_eq!(fs::read(&target).unwrap(), raw);
}

#[test]
fn artifact_digest_reads_current_bytes_and_rejects_non_files() {
    use super::recovery::clone::native::digest;

    let fixture = Fixture::new();
    let path = fixture.root.join("unregistered-artifact.bin");
    let directory_error = digest::file(&fixture.root).unwrap_err().to_string();
    assert!(
        directory_error.contains("复制证据必须是普通文件"),
        "{directory_error}"
    );
    let absent_error = digest::file(&path).unwrap_err().to_string();
    assert!(absent_error.contains("无法读取复制证据"), "{absent_error}");

    for (content, bytes, sha256) in [
        (
            b"first".as_slice(),
            5,
            "a7937b64b8caa58f03721bb6bacf5c78cb235febe0e70b1b84cd99541461a08e",
        ),
        (
            b"changed and longer".as_slice(),
            18,
            "1cc02c98c3cfaa5074a27f6eac65507dd3a9311d755cdf64c1a4698dc4a55b26",
        ),
    ] {
        fs::write(&path, content).unwrap();
        let actual = digest::file(&path).unwrap();
        assert_eq!(actual.bytes, bytes);
        assert_eq!(actual.sha256, sha256);
    }
}

#[cfg(windows)]
#[test]
fn document_and_artifact_read_use_real_windows_paths_beyond_max_path() {
    use super::recovery::clone::native::digest;
    use std::os::windows::ffi::OsStrExt;

    let fixture = Fixture::new();
    let native_backend = fixture.backend.canonicalize().unwrap();
    let native_root = fixture.root.canonicalize().unwrap();
    let mut directory = native_root.clone();
    while directory.as_os_str().encode_wide().count() <= 280 {
        directory.push("恢复证据 长目录".repeat(4));
    }
    fs::create_dir_all(&directory).unwrap();
    let native = directory.join("document.json");
    let portable = fixture
        .root
        .join(native.strip_prefix(&native_root).unwrap());
    assert!(portable.as_os_str().encode_wide().count() > 260);
    let raw = br#"{"value":7}"#;
    fs::write(&native, raw).unwrap();
    let artifact = digest::file(&native).unwrap();
    assert_eq!(artifact.bytes, raw.len() as u64);

    for (path, backend) in [(&native, &native_backend), (&portable, &fixture.backend)] {
        let observed = document::read(path, backend, "长路径恢复文档").unwrap();
        assert_eq!(observed.raw.as_slice(), raw);
        assert_eq!(observed.value, json!({"value":7}));
        assert_eq!(observed.sha256(), artifact.sha256);
    }
}

#[test]
fn reference_configuration_drift_rejects_before_publication() {
    let fixture = Fixture::new();
    let mut options = fixture.reference_options(None);
    options.work_dir = fixture.root.join("work").join("configuration-drift");
    for directory in [
        fixture.root.join("source-runtime").join("config"),
        fixture.root.join("config"),
    ] {
        let path = directory.join("app.test.toml");
        let original = fs::read(&path).unwrap();
        fs::write(&path, b"[object_storage]\nsecret_key='changed'\n").unwrap();
        assert!(reference::build(&fixture.backend, &options).is_err());
        assert!(!fixture.output.exists());
        assert!(!options.work_dir.exists());
        fs::write(path, original).unwrap();
    }
    reference::build(&fixture.backend, &options).unwrap();
}

#[test]
fn source_reference_projection_rejects_binding_runtime_and_configuration_drift() {
    let fixture = Fixture::new();
    let path = fixture.root.join("source-request.json");
    let request = document::read(&path, &fixture.backend, "私有源请求").unwrap();
    let descriptor = document::binding(&request);
    let mut bad = descriptor.clone();
    bad["sha256"] = Value::String("0".repeat(64));
    assert!(reference::source_projection(&fixture.backend, &bad).is_err());
    bad = descriptor.clone();
    bad["unexpected"] = Value::Bool(true);
    assert!(reference::source_projection(&fixture.backend, &bad).is_err());

    let runtime = std::path::Path::new(request.value["runtime"]["path"].as_str().unwrap());
    let runtime_before = fs::read(runtime).unwrap();
    fs::write(runtime, b"{}").unwrap();
    assert!(reference::source_projection(&fixture.backend, &descriptor).is_err());
    fs::write(runtime, &runtime_before).unwrap();

    let config = fixture.root.join("source-runtime/config/app.test.toml");
    let config_before = fs::read(&config).unwrap();
    let mut changed = config_before.clone();
    changed.push(b'\n');
    fs::write(&config, changed).unwrap();
    assert!(reference::source_projection(&fixture.backend, &descriptor).is_err());
    fs::write(&config, &config_before).unwrap();

    let mut wrong = request.value.clone();
    wrong["source"]["s3"]["configuration"]["sha256"] =
        Value::String("changed-private-value".into());
    fs::write(&path, serde_json::to_vec(&wrong).unwrap()).unwrap();
    assert!(reference::source_projection(&fixture.backend, &descriptor).is_err());
    let rebound =
        document::binding(&document::read(&path, &fixture.backend, "私有源请求").unwrap());
    let error = reference::source_projection(&fixture.backend, &rebound)
        .unwrap_err()
        .to_string();
    assert!(error.contains("应用配置绑定摘要已变化"));
    assert!(!error.contains("changed-private-value"));
    fs::write(&path, &request.raw).unwrap();
    reference::source_projection(&fixture.backend, &descriptor).unwrap();
    assert_eq!(fs::read(runtime).unwrap(), runtime_before);
    assert_eq!(fs::read(config).unwrap(), config_before);
    assert_eq!(fs::read(path).unwrap(), request.raw);
}

#[test]
fn target_projection_rejects_descriptor_credentials_and_physical_mapping_changes() {
    let fixture = Fixture::new();
    let chain = chain::load(
        &fixture.backend,
        &fixture.reference_plan,
        &fixture.backup_receipt,
        &fixture.comparison_sources,
        &fixture.arm_input,
        &fixture.fresh_verify,
        "base",
    )
    .unwrap();
    for field in ["scope_id", "s3", "databases"] {
        let mut reference = fixture.reference_value.clone();
        reference["target"][field] = match field {
            "scope_id" => json!("another-target"),
            "s3" => reference["source"]["s3"].clone(),
            _ => {
                let mut databases = reference["target"]["databases"].clone();
                databases[0]["database"] = json!("another_physical_database");
                databases
            }
        };
        assert!(
            chain::validate_links(
                &reference,
                &chain.backup,
                &chain.comparison,
                &chain.arm,
                &chain.fresh
            )
            .is_err()
        );
        assert!(!fixture.output.exists());
    }
    let path = &chain.arm.target.document.path;
    let original = std::fs::read(path).unwrap();
    let mut request = read_value(path);
    request["target"]["s3"]["secret_key"] = json!("changed-target-private-secret");
    std::fs::write(path, serde_json::to_vec(&request).unwrap()).unwrap();
    let document = document::read(path, &fixture.backend, "测试目标请求").unwrap();
    let target = target::validate(&fixture.backend, document, "base").unwrap();
    let before = std::fs::read(path).unwrap();
    let error = reference_configuration::TargetProjection::load(&target)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("object_storage.secret_key与绑定私有请求不一致"));
    assert!(!error.contains("changed-target-private-secret"));
    assert!(!error.contains("target-secret"));
    assert_eq!(std::fs::read(path).unwrap(), before);
    assert!(!fixture.output.exists());
    std::fs::write(path, original).unwrap();
}

#[test]
fn source_identity_rejects_changed_reference_and_private_credentials_without_writes() {
    let fixture = Fixture::new();
    let receipt = read_value(&fixture.backup_receipt);
    let export = &receipt["result"]["source_export"];
    let generation = &receipt["result"]["source_generation"];
    backup::validate_source_identity(
        &fixture.backend,
        &fixture.reference_value,
        export,
        generation,
    )
    .unwrap();
    for (section, field, replacement) in [
        ("source", "scope_id", json!("another-source")),
        (
            "tools",
            "mysql",
            json!({"path":fixture.root.join("other.exe"),"sha256":"a".repeat(64)}),
        ),
        (
            "tools",
            "mysqldump",
            json!({"path":fixture.reference_value["tools"]["mysqldump"]["path"],"sha256":"a".repeat(64)}),
        ),
        (
            "source",
            "s3",
            fixture.reference_value["target"]["s3"].clone(),
        ),
    ] {
        let mut changed = fixture.reference_value.clone();
        changed[section][field] = replacement;
        assert!(
            backup::validate_source_identity(&fixture.backend, &changed, export, generation)
                .is_err()
        );
        assert!(!fixture.output.exists());
    }
    let path = fixture.root.join("source-request.json");
    let original = std::fs::read(&path).unwrap();
    let mut request = read_value(&path);
    request["source"]["s3"]["configuration"]["sha256"] = json!("different-private-secret");
    std::fs::write(&path, serde_json::to_vec(&request).unwrap()).unwrap();
    let mut changed = export.clone();
    changed["source_request"] = binding(&path);
    changed.as_object_mut().unwrap().remove("identity_sha256");
    changed["identity_sha256"] = json!(hash(&changed));
    let before = std::fs::read(&path).unwrap();
    let error = backup::validate_source_identity(
        &fixture.backend,
        &fixture.reference_value,
        &changed,
        generation,
    )
    .err()
    .unwrap()
    .to_string();
    assert!(error.contains("应用配置绑定摘要已变化"));
    assert!(!error.contains("different-private-secret"));
    assert!(!error.contains("source-secret"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(!fixture.output.exists());
    std::fs::write(path, original).unwrap();
}

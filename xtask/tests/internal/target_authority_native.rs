use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use crate::recovery::clone::native::{
    digest, json as json_io,
    target_verify::{authority, tree},
};

// 结构边界使用最小文件夹具；完整历史组合复用既有published夹具。
struct Fixture {
    backend: PathBuf,
    _directory: tempfile::TempDir,
    run: PathBuf,
    target: PathBuf,
    value: Value,
}

impl Fixture {
    fn new() -> Self {
        let backend = crate::workspace::root_dir();
        let parent = backend.join(".local-tests/target-authority-native");
        fs::create_dir_all(&parent).unwrap();
        let directory = tempfile::tempdir_in(&parent).unwrap();
        let root = directory.path();
        let run = root.join("run");
        let target = root.join("target");
        fs::create_dir(&run).unwrap();
        fs::create_dir(&target).unwrap();
        fs::create_dir(target.join("empty-directory")).unwrap();
        fs::write(target.join("copy-target.guard"), [0]).unwrap();
        for name in [
            "source-request.json",
            "environment.json",
            "request.json",
            "initialized-files.json",
        ] {
            write(&root.join(name), &json!({"structural_fixture": true}));
        }
        write(
            &target.join("request.json"),
            &json!({"structural_fixture": true}),
        );
        write(
            &target.join("prepare.json"),
            &json!({"request": bound(&root.join("request.json"))}),
        );
        let generation = json!({"scope_id": "authority-fixture", "storage": {"fixture": true}});
        write(
            &target.join("initialized.json"),
            &json!({"generation": generation}),
        );
        write(
            &run.join("manifest.json"),
            &json!({
                "format_version": 1, "kind": "devex-clone-run", "id": "authority-fixture",
                "source_request": bound(&root.join("source-request.json")), "source_export": null,
                "initialized": bound(&target.join("initialized.json")),
                "source_environment": bound(&root.join("environment.json")),
                "target_environment": bound(&root.join("environment.json")),
                "copy_directory": root.join("copy"), "copy_stage": "source_to_seed", "build_bridges": [],
            }),
        );
        let value = json!({
            "format_version": 1, "kind": "devex-clone-target-authority",
            "initialized": bound(&target.join("initialized.json")),
            "request": bound(&root.join("request.json")),
            "initialized_files": bound(&root.join("initialized-files.json")),
            "files": tree::snapshot(&backend, &target, false).unwrap(),
            "generation": {"scope_id": "authority-fixture"},
            "remote_writes": 0, "target_ready": false, "clone_verified": false, "restore_qualified": false,
        });
        Self {
            backend,
            _directory: directory,
            run,
            target,
            value,
        }
    }

    fn validate(&self) -> crate::Result<()> {
        authority::validate(&self.backend, &self.run, &self.value)
    }
}

fn bound(path: &Path) -> Value {
    let digest = digest::file(path).unwrap();
    json!({"path": path, "bytes": digest.bytes, "sha256": digest.sha256})
}

fn write(path: &Path, value: &Value) {
    json_io::write_new(path, value).unwrap();
}

#[test]
fn published_auxiliary_paths_protect_the_real_manifest_and_original_fixture() {
    use crate::recovery::{
        clone::native::{
            bridge::scope::Scope,
            target_verify::{guard, input::TargetContext},
        },
        fixture_control::source_pair::execution::tests::Fixture as Published,
    };
    let published = Published::new();
    let publication = published.publication();
    let backend = published.output.join("backend");
    let workspace = backend.join(".local-tests/authority-inputs");
    let target = workspace.join("target");
    fs::create_dir_all(&target).unwrap();
    let receipt = workspace.join("receipt.json");
    write(&receipt, &json!({}));
    for name in [
        "initialized-files.json",
        "prepared-files.json",
        "registration.json",
    ] {
        write(&workspace.join(name), &json!({}));
    }
    let build = workspace.join("build.json");
    write(&build, &json!({"artifacts":{}}));
    let launch = workspace.join("launch.json");
    write(
        &launch,
        &json!({"credential_files":{"access_key":bound(&receipt),"secret_key":bound(&receipt)}}),
    );
    let tool = published.backend.join(".local-tests/mysql-tool.bin");
    fs::write(&tool, b"external tool fixture").unwrap();
    let mut review = bound(&receipt);
    review["canonical_sha256"] = json!("0".repeat(64));
    let request = json!({"execution_backend":{"path":backend,"execution_source":published.binding()},
        "review":review,"maintenance_build":bound(&build),"tools":{"mysql":{"path":tool,"sha256":digest::file(&tool).unwrap().sha256}},
        "target":{"databases":[{"defaults_file":receipt}]},
        "storage":{"rustfs":{"identity":{"executable":tool},"process_receipt":bound(&receipt),"launch_receipt":bound(&launch)},
            "redis":{"wsl":{"path":tool},"configuration":bound(&receipt)}}});
    let manifest = json!({"build_bridges":[],
        "source_environment":bound(&receipt),"target_environment":bound(&receipt)});
    write(&workspace.join("manifest.json"), &manifest);
    let mut context = TargetContext {
        backend: backend.clone(),
        run_dir: workspace.clone(),
        manifest,
        target_dir: target,
        initialized_path: receipt.clone(),
        initialized_binding: json!({}),
        initialized: json!({}),
        request_path: receipt,
        request,
        environment: Default::default(),
        observation_dir: workspace.clone(),
    };
    // 这里只核验路径收集和文件保护，不把结构夹具声称为真实服务或维护证明。
    let authority =
        json!({"files":[],"generation":{"configuration":{"directory":workspace,"files":{}}}});
    let paths = authority::protected_paths(&context, &authority).unwrap();
    let original = PathBuf::from(
        publication["original"]["fixture_receipt"]["path"]
            .as_str()
            .unwrap(),
    );
    let manifest = published.output.join("execution-source.json");
    for expected in [&original, &manifest, &tool] {
        assert!(
            paths
                .iter()
                .any(|p| fs::canonicalize(p).unwrap() == fs::canonicalize(expected).unwrap())
        );
    }
    let scope = Scope::load(&backend, &[], &[]).unwrap();
    let protected = guard::Inputs::open(paths).unwrap();
    protected.check().unwrap();
    scope.assert_unchanged().unwrap();
    let bytes = fs::read(&original).unwrap();
    if fs::write(&original, b"drift original receipt").is_ok() {
        assert!(protected.check().is_err());
    } else {
        assert_eq!(fs::read(&original).unwrap(), bytes);
    }
    drop(protected);
    fs::write(&original, bytes).unwrap();
    context.request["execution_backend"]["fixture"] = json!({});
    assert!(authority::protected_paths(&context, &authority).is_err());
}

#[test]
fn authority_checks_actual_files_and_preserves_empty_directories_without_writes() {
    let fixture = Fixture::new();
    let before = tree::snapshot(&fixture.backend, fixture._directory.path(), false).unwrap();
    fixture.validate().unwrap();
    assert_eq!(
        tree::snapshot(&fixture.backend, fixture._directory.path(), false).unwrap(),
        before
    );
    assert!(
        fixture.value["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["path"] == "empty-directory")
    );
}

#[test]
fn authority_rejects_qualifications_writes_and_coerced_boolean_values() {
    let fixture = Fixture::new();
    for (field, invalid) in [
        ("format_version", json!(true)),
        ("remote_writes", json!(true)),
        ("remote_writes", json!(1)),
        ("target_ready", json!(true)),
        ("clone_verified", json!(0)),
        ("restore_qualified", json!(null)),
    ] {
        let mut value = fixture.value.clone();
        value[field] = invalid;
        assert!(
            authority::validate(&fixture.backend, &fixture.run, &value).is_err(),
            "{field}"
        );
    }
    let mut value = fixture.value.clone();
    value["unknown"] = json!(false);
    assert!(authority::validate(&fixture.backend, &fixture.run, &value).is_err());
}

#[test]
fn authority_rejects_source_generation_and_complete_file_tree_drift() {
    let mut fixture = Fixture::new();
    fixture.value["generation"]["scope_id"] = json!("other-scope");
    assert!(fixture.validate().is_err());
    fixture.value["generation"]["scope_id"] = json!("authority-fixture");
    fixture.validate().unwrap();
    fs::remove_dir(fixture.target.join("empty-directory")).unwrap();
    assert!(fixture.validate().is_err());
}

#[test]
fn authority_rejects_rebound_request_and_foreign_lineage_paths() {
    let mut fixture = Fixture::new();
    let other = fixture._directory.path().join("other.json");
    write(&other, &json!({"structural_fixture": true}));
    fixture.value["request"] = bound(&other);
    assert!(fixture.validate().is_err());
    fixture.value["request"] = bound(&fixture._directory.path().join("request.json"));
    fixture.value["initialized_files"] = bound(&other);
    assert!(fixture.validate().is_err());
}

#[test]
fn authority_rejects_modified_initialized_and_raw_target_request() {
    let fixture = Fixture::new();
    fs::write(fixture.target.join("request.json"), b"{\"changed\":true}").unwrap();
    assert!(fixture.validate().is_err());
    fs::write(
        fixture.target.join("initialized.json"),
        b"{\"changed\":true}",
    )
    .unwrap();
    assert!(fixture.validate().is_err());
}

#[test]
fn authority_rejects_missing_guard_before_a_controller_can_create_it() {
    let mut fixture = Fixture::new();
    fs::remove_file(fixture.target.join("copy-target.guard")).unwrap();
    fixture.value["files"] = tree::snapshot(&fixture.backend, &fixture.target, false).unwrap();
    let before = fixture.value["files"].clone();
    assert!(
        fixture
            .validate()
            .unwrap_err()
            .to_string()
            .contains("规范控制互斥文件")
    );
    assert_eq!(
        tree::snapshot(&fixture.backend, &fixture.target, false).unwrap(),
        before
    );
    assert!(!fixture.target.join("copy-target.guard").exists());
}

#[test]
fn authority_rejects_residual_lock_without_removing_its_contents() {
    let mut fixture = Fixture::new();
    let lock = fixture.target.join("initialize.lock");
    fs::create_dir(&lock).unwrap();
    fs::write(lock.join("unknown.json"), b"retain").unwrap();
    fixture.value["files"] = tree::snapshot(&fixture.backend, &fixture.target, false).unwrap();
    let before = fixture.value["files"].clone();
    assert!(
        fixture
            .validate()
            .unwrap_err()
            .to_string()
            .contains("初始化锁仍存在")
    );
    assert_eq!(
        tree::snapshot(&fixture.backend, &fixture.target, false).unwrap(),
        before
    );
    assert_eq!(fs::read(lock.join("unknown.json")).unwrap(), b"retain");
}

fn observed_authority(
    fixture: &crate::target_verify_fixture::Fixture,
    port: &mut dyn FnMut(&str) -> crate::Result<()>,
) -> crate::Result<Value> {
    use crate::recovery::clone::native::maintenance;
    let stage = crate::recovery::clone::native::lifecycle::stage::StageRequest {
        attempt: None,
        ..fixture.stage()
    };
    authority::observe_with(
        &fixture.backend,
        &stage,
        |backend, request, environment, scope| {
            authority::generation::observe(
                backend,
                request,
                environment,
                &mut |root, path, receipt| {
                    // 只替代当前工具链版本观察；文件、完整来源与冻结维护产物仍实际核对。
                    maintenance::verify_evidence(root, path, Some(receipt))?;
                    scope.verify_source(
                        root,
                        path,
                        receipt,
                        receipt["source"]["worktree_fingerprint"].as_str().unwrap(),
                        None,
                    )?;
                    Ok(receipt.clone())
                },
                port,
            )
        },
    )
}

#[test]
fn native_authority_recomputes_the_generation_and_keeps_complete_history_readonly() {
    let fixture = crate::published_target_fixture::complete();
    let manifest_path = fixture.run.join("manifest.json");
    #[cfg(windows)]
    {
        let mut manifest = json_io::read(&manifest_path).unwrap();
        manifest["initialized"]["path"] = json!(
            fixture
                .target
                .join("initialized.json")
                .to_str()
                .unwrap()
                .replace('\\', "/")
        );
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
    }
    let manifest = json_io::read(&manifest_path).unwrap();
    let before = tree::snapshot(&fixture.backend, &fixture.local, false).unwrap();
    let mut ports = Vec::new();
    let value = observed_authority(&fixture, &mut |url| {
        ports.push(url.to_owned());
        Ok(())
    })
    .unwrap();
    assert_eq!(
        ports,
        [
            "http://127.0.0.1:28080",
            "http://127.0.0.1:28081/readyz",
            "http://127.0.0.1:28082"
        ]
    );
    assert_eq!(value["initialized"], manifest["initialized"]);
    assert_eq!(value["request"], bound(&fixture.local.join("request.json")));
    let mut expected =
        json_io::read(&fixture.target.join("initialized.json")).unwrap()["generation"].clone();
    expected.as_object_mut().unwrap().remove("storage");
    assert_eq!(value["generation"], expected);
    assert_eq!(value["remote_writes"], 0);
    for field in ["target_ready", "clone_verified", "restore_qualified"] {
        assert_eq!(value[field], false);
    }
    assert_eq!(
        tree::snapshot(&fixture.backend, &fixture.local, false).unwrap(),
        before
    );
    let error = observed_authority(&fixture, &mut |_| Err("明确观察失败".into())).unwrap_err();
    assert!(error.to_string().contains("明确观察失败"));
    assert_eq!(
        tree::snapshot(&fixture.backend, &fixture.local, false).unwrap(),
        before
    );
}

#[test]
fn native_authority_rejects_actual_config_start_history_and_changes_during_observation() {
    let fixture = crate::published_target_fixture::complete();
    let config = Path::new(fixture.request["config_dir"].as_str().unwrap()).join("app.test.toml");
    let original = fs::read(&config).unwrap();
    let mut changed = original.clone();
    changed.extend_from_slice(b"\n# drift\n");
    fs::write(&config, &changed).unwrap();
    let mut calls = 0;
    assert!(
        observed_authority(&fixture, &mut |_| {
            calls += 1;
            Ok(())
        })
        .is_err()
    );
    assert_eq!(calls, 0);
    assert_eq!(fs::read(&config).unwrap(), changed);
    fs::write(&config, original).unwrap();
    let selected =
        json_io::read(&fixture.target.join("initialized.json")).unwrap()["generation"]["selected"]
            .clone();
    let runtime = Path::new(selected["runtime_dir"].as_str().unwrap());
    fs::create_dir(runtime).unwrap();
    let producer = runtime.join("api.json");
    fs::write(&producer, b"unknown producer must survive").unwrap();
    assert!(
        observed_authority(&fixture, &mut |_| {
            calls += 1;
            Ok(())
        })
        .is_err()
    );
    assert_eq!(calls, 0);
    assert_eq!(
        fs::read(&producer).unwrap(),
        b"unknown producer must survive"
    );
    fs::remove_file(producer).unwrap();
    fs::remove_dir(runtime).unwrap();
    let extra = fixture.target.join("unknown-during-observation.json");
    assert!(
        observed_authority(&fixture, &mut |_| {
            if !extra.exists() {
                fs::write(&extra, b"retain").unwrap();
            }
            Ok(())
        })
        .is_err()
    );
    assert_eq!(fs::read(extra).unwrap(), b"retain");
    assert!(!fixture.target.join("initialize.lock").exists());
}

fn register_arm_manifest(fixture: &crate::target_verify_fixture::Fixture) -> Value {
    let registration_path = fixture.local.join("registration.json");
    let registration = json_io::read(&registration_path).unwrap();
    let storage = &registration["storage_run"];
    let run = Path::new(storage["path"].as_str().unwrap());
    // 这里验证登记权威，独立存储绑定只需满足清单的静态证据契约。
    let evidence = (0..10)
        .map(|index| {
            let path = run.join(format!("arm-authority-{index}.json"));
            fs::write(
                &path,
                serde_json::to_vec(&json!({"fixture":index})).unwrap(),
            )
            .unwrap();
            bound(&path)
        })
        .collect::<Vec<_>>();
    let path = fixture.run.join("manifest.json");
    let mut manifest = json_io::read(&path).unwrap();
    manifest["copy_stage"] = json!("seed_to_arm");
    manifest["source_registration"] = manifest["source_request"].clone();
    manifest["target_registration"] = bound(&registration_path);
    manifest["target_initialized_files"] = bound(&fixture.local.join("initialized-files.json"));
    manifest["target_environment"] = registration["environment"].clone();
    manifest["target_storage_run"] = json!({
        "path":storage["path"],"manifest":storage["manifest"],
        "storage":{"request":evidence[0],"storage":{"identity":{"pid":1},"sha256":"a".repeat(64),
            "process_receipt":evidence[1],"launch_receipt":evidence[2]},"attempt":1,
            "data_directory":run.join("objects"),"api_url":"http://127.0.0.1:29000",
            "console_url":"http://127.0.0.1:29001","restart_result":evidence[3]},
        "cache":{"request":evidence[4],"runtime":evidence[5],"redis":{"pid":2},
            "owner":evidence[6],"controller":evidence[7],"attempt":2,
            "ready":evidence[8],"restart_result":evidence[9]}
    });
    json_io::write_replace(&path, &manifest).unwrap();
    manifest
}

fn rejected_arm_authority(fixture: &crate::target_verify_fixture::Fixture, reason: &str) {
    let mut observations = 0;
    let error = observed_authority(fixture, &mut |_| {
        observations += 1;
        Ok(())
    })
    .unwrap_err();
    assert!(error.to_string().contains(reason), "{error}");
    assert_eq!(observations, 0);
}

#[test]
fn native_arm_authority_binds_the_initialized_workspace_and_requires_the_actual_arm_side() {
    use crate::target_verify_fixture::{Fixture as PublishedFixture, snapshot};

    let fixture = PublishedFixture::for_arm_authority();
    let manifest = register_arm_manifest(&fixture);
    let manifest_path = fixture.run.join("manifest.json");
    let before = snapshot(&fixture.local);
    let mut observations = 0;
    let value = observed_authority(&fixture, &mut |_| {
        observations += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!(observations, 3);
    assert_eq!(fixture.request["side"], "base");
    assert_eq!(
        value["initialized_files"],
        manifest["target_initialized_files"]
    );
    assert_eq!(value["remote_writes"], 0);
    authority::validate(&fixture.backend, &fixture.run, &value).unwrap();
    assert_eq!(snapshot(&fixture.local), before);
    let external = Path::new(manifest["target_storage_run"]["path"].as_str().unwrap());
    assert!(!external.join("run.lock").exists());

    for (field, name) in [
        ("target_registration", "registration.json"),
        ("target_initialized_files", "initialized-files.json"),
    ] {
        let foreign = fixture.local.join(format!("foreign-{name}"));
        fs::copy(fixture.local.join(name), &foreign).unwrap();
        let mut changed = manifest.clone();
        changed[field] = bound(&foreign);
        json_io::write_replace(&manifest_path, &changed).unwrap();
        rejected_arm_authority(&fixture, "未绑定初始化工作区的原登记和文件谱系");
        assert!(authority::validate(&fixture.backend, &fixture.run, &value).is_err());
    }

    let mut changed = manifest;
    changed["target_environment"] = bound(&fixture.local.join("target-environment.json"));
    json_io::write_replace(&manifest_path, &changed).unwrap();
    rejected_arm_authority(&fixture, "目标原登记、环境、请求或文件谱系不同");
    assert!(authority::validate(&fixture.backend, &fixture.run, &value).is_err());

    let seed = PublishedFixture::for_status();
    register_arm_manifest(&seed);
    rejected_arm_authority(&seed, "只接受实际 base 或 candidate 请求");
}

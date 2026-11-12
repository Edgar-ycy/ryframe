use super::*;

fn fixture() -> tempfile::TempDir {
    let base = crate::workspace::root_dir().join(".local-tests");
    std::fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix("dataset-authority-unit-")
        .tempdir_in(base)
        .unwrap()
}

#[test]
fn browser_authority_projects_bound_files_and_the_selected_target() {
    let directory = fixture();
    let backend = crate::workspace::root_dir();
    let mut inputs = inputs::Inputs::new(&backend);
    let mut read = |name: &str, raw: &[u8]| {
        let path = directory.path().join(name);
        std::fs::write(&path, raw).unwrap();
        inputs.read(&path).unwrap()
    };
    let runtime = read("runtime.json", b"{ \"kind\": \"restore-runtime\" }\n");
    let target = read("target.json", b"{\"kind\":\"restore-target-plan\"}\n");
    let stop = read(
        "stop.json",
        b"{\"status\":\"seed_source_generation_published\"}\n",
    );
    let lineage = read("lineage.json", b"{\"kind\":\"dataset-lineage\"}\n");
    let dataset = json!({
        "source_generation": document::binding(&stop),
        "dataset_lineage": document::binding(&lineage),
    });
    let selected = json!({
        "scope_id": "recovery-b1",
        "api_url": "http://127.0.0.1:25001",
        "frontend_url": "http://127.0.0.1:25002",
        "private": "not for the browser",
    });
    let execution = json!(directory.path().join("execution-backend"));
    let result = project(runtime.clone(), &target, &dataset, &selected, &execution);
    assert_eq!(result.runtime, runtime.value);
    assert_eq!(
        result.authority,
        json!({
            "format_version": 1,
            "kind": "restore-dataset-authority",
            "runtime": document::binding(&runtime),
            "target_plan": document::binding(&target),
            "source_generation": document::binding(&stop),
            "dataset_lineage": document::binding(&lineage),
            "target": {
                "scope_id": "recovery-b1",
                "api_url": "http://127.0.0.1:25001",
                "frontend_url": "http://127.0.0.1:25002",
            },
            "execution_backend": execution,
        }),
    );
    inputs.finish(Ok(())).unwrap();
}

#[test]
fn browser_authority_rejects_forged_receipt_without_publishing() {
    let directory = fixture();
    let backend = crate::recovery::fixture_control::git_root(
        &crate::workspace::root_dir(),
        "恢复数据集权威测试根",
    )
    .unwrap();
    let runtime = directory.path().join("runtime.json");
    let target = directory.path().join("target.json");
    let forged =
        br#"{"format_version":3,"kind":"restore-runtime","authority":{"remote_writes":0}}"#;
    std::fs::write(&runtime, forged).unwrap();
    std::fs::write(&target, b"{}").unwrap();
    let failure = request(&json!({
        "backend_dir":backend,"runtime_receipt":runtime,"target_plan":target,
    }))
    .unwrap_err();
    assert!(failure.to_string().contains("恢复运行收据"), "{failure}");
    assert_eq!(std::fs::read(runtime).unwrap(), forged);
    assert_eq!(std::fs::read(target).unwrap(), b"{}");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
}

#[test]
fn parallel_and_protected_serial_source_scans_preserve_complete_results_and_guards() {
    use crate::frontend_origin::{manifest, sources};
    use crate::recovery::clone::native::source;
    let directory = fixture();
    let root = directory.path();
    std::fs::create_dir(root.join("xtask")).unwrap();
    std::fs::write(root.join(sources::POLICY), br#"{"format_version":1,"files":["xtask/source-inventory.json"],"trees":["inputs"],"excluded_directories":[]}"#).unwrap();
    std::fs::create_dir(root.join("inputs")).unwrap();
    for index in 0..17 {
        std::fs::write(
            root.join(format!("inputs/来源-{index:02}.txt")),
            format!("input-{index}"),
        )
        .unwrap();
    }
    for arguments in [
        vec!["init", "--quiet"],
        vec!["add", "."],
        vec![
            "-c",
            "user.name=source-unit",
            "-c",
            "user.email=source-unit@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "-m",
            "fixture",
        ],
    ] {
        let mut command = std::process::Command::new("git");
        command.arg("-C").arg(root).args(arguments);
        for (name, _) in std::env::vars_os() {
            if name
                .to_string_lossy()
                .to_ascii_uppercase()
                .starts_with("GIT_")
            {
                command.env_remove(name);
            }
        }
        assert!(
            command
                .env("GIT_OPTIONAL_LOCKS", "0")
                .status()
                .unwrap()
                .success()
        );
    }
    let parallel_source = source::capture(root).unwrap();
    let parallel_manifest = manifest::capture(root, true).unwrap();
    assert_eq!(parallel_source.files.len(), 18);
    assert_eq!(parallel_manifest.as_array().unwrap().len(), 18);
    let window = reads::Window::begin().unwrap();
    assert!(reads::is_active());
    assert_eq!(source::capture(root).unwrap(), parallel_source);
    assert_eq!(
        manifest::capture(root, !reads::is_active()).unwrap(),
        parallel_manifest
    );
    // finish 会取出读取集合并再次采集完整来源；窗口仍须保持串行和完整保护。
    window.finish(Ok(())).unwrap();
    let changed = std::fs::write(root.join("inputs/来源-00.txt"), b"changed");
    let checked = window.finish(Ok(()));
    assert_eq!(checked.is_err(), changed.is_ok());
    drop(window);
    assert!(!reads::is_active());
    std::fs::write(
        root.join(sources::POLICY),
        br#"{"format_version":1,"files":["missing"],"trees":[],"excluded_directories":[]}"#,
    )
    .unwrap();
    assert!(manifest::capture(root, true).is_err());
    let window = reads::Window::begin().unwrap();
    assert!(manifest::capture(root, !reads::is_active()).is_err());
    assert!(window.finish(Ok(())).is_ok());
}

#[test]
fn delegated_json_read_remains_protected_after_reader_returns() {
    let directory = fixture();
    let file = directory.path().join("input.json");
    std::fs::write(&file, b"{\"value\":1}").unwrap();
    let window = reads::Window::begin().unwrap();
    strict::read(&file).unwrap();
    let changed = std::fs::write(&file, b"{\"value\":2}");
    let result = window.finish(Ok(()));
    if changed.is_ok() {
        assert!(result.is_err());
    } else {
        assert!(result.is_ok());
    }
}

#[test]
fn optional_configuration_appearance_and_directory_additions_are_rejected() {
    let directory = fixture();
    let optional = directory.path().join("tools.toml");
    let window = reads::Window::begin().unwrap();
    reads::optional(&optional).unwrap();
    std::fs::write(&optional, b"[verification]").unwrap();
    assert!(window.finish(Ok(())).is_err());
    drop(window);
    let window = reads::Window::begin().unwrap();
    reads::directory(directory.path()).unwrap();
    std::fs::write(directory.path().join("unexpected.json"), b"{}").unwrap();
    assert!(window.finish(Ok(())).is_err());
}

#[test]
fn native_frontend_tool_binding_rejects_coordinator_or_binary_drift() {
    let directory = fixture();
    let executable = directory.path().join("registered.exe");
    let alternate = directory.path().join("alternate.exe");
    for file in [&executable, &alternate] {
        std::fs::write(file, b"same bytes").unwrap();
    }
    let observed = digest::file(&executable).unwrap();
    let current = json!({"coordinator_root":directory.path(),"coordinator_head":"a".repeat(40),
        "inventory_sha256":"b".repeat(64),"frontend_server":{"path":executable,
            "bytes":observed.bytes,"sha256":observed.sha256}});
    commands::validate_tools(&current, &current).unwrap();
    let mut recorded = current.clone();
    recorded["frontend_server"]["path"] = json!(alternate);
    assert!(commands::validate_tools(&recorded, &current).is_err());
    let mut recorded = current.clone();
    recorded["frontend_server"]["sha256"] = json!("c".repeat(64));
    assert!(commands::validate_tools(&recorded, &current).is_err());
    let mut recorded = current.clone();
    recorded["coordinator_head"] = json!("d".repeat(40));
    assert!(commands::validate_tools(&recorded, &current).is_err());
    let mut recorded = current.clone();
    recorded["inventory_sha256"] = json!("e".repeat(64));
    assert!(commands::validate_tools(&recorded, &current).is_err());
}

#[test]
fn native_frontend_argv_binds_receipt_and_only_allows_the_exact_b0_contract() {
    let directory = fixture();
    let expected = directory.path().join("expected.exe");
    let different = directory.path().join("different.exe");
    std::fs::write(&expected, b"fixture-a").unwrap();
    std::fs::write(&different, b"fixture-b").unwrap();
    let mut request = json!({"runtime_directory":directory.path(),"authority":{"frontend_endpoint":"http://127.0.0.1:15174",
        "api_endpoint":"http://127.0.0.1:18180/readyz",
        "frontend_sha":"a".repeat(40),"backend_product_sha":"b".repeat(40),
        "backend_execution_sha":"b".repeat(40),"backend_adapter_contract":null},
        "roots":{"frontend":directory.path()},"paths":{"frontend_build":directory.path().join("build.json")},
        "digests":{"frontend_build":"c".repeat(64)}});
    let executable = json!(expected);
    let frontend = commands::frontend_command(&request, &executable).unwrap();
    assert_eq!(
        frontend,
        json!([
            executable,
            "__restore-frontend-serve",
            "--frontend",
            directory.path(),
            "--receipt",
            request["paths"]["frontend_build"],
            "--receipt-sha256",
            "c".repeat(64),
            "--expected-head",
            "a".repeat(40),
            "--host",
            "127.0.0.1",
            "--port",
            "15174",
            "--runtime-dir",
            directory.path(),
            "--api-port",
            "18180"
        ])
    );
    let expected = json!({"api":[executable,"--flag"],"worker":[executable],"frontend":frontend});
    let mut actual = expected.clone();
    actual["frontend"][0] = json!(different);
    assert!(commands::recorded_spelling(expected, &actual).is_err());
    request["authority"]["frontend_sha"] = json!(crate::recovery::frontend_build::b0::HEAD);
    assert!(commands::frontend_command(&request, &executable).is_err());
    request["authority"]["backend_product_sha"] = json!("815c5eafb09d4b493319d255fb7ab88ddd02c8b6");
    request["authority"]["backend_execution_sha"] =
        json!("c05114bcdf5c369cd74087db6317ce3c8f89bee8");
    request["authority"]["backend_adapter_contract"] = json!("legacy-stable-readiness-b0-v1");
    let b0 = commands::frontend_command(&request, &executable).unwrap();
    assert_eq!(b0[9], request["authority"]["frontend_sha"]);
    assert_eq!(b0[18], "--b0");
    request["authority"]["frontend_endpoint"] = json!("http://[::1]:15174");
    assert!(commands::frontend_command(&request, &executable).is_err());
}

#[test]
fn unknown_arguments_and_duplicate_fields_cannot_open_a_read_window() {
    assert!(
        request(&json!({"backend_dir":"x","runtime_receipt":"x","target_plan":"x","extra":true}))
            .is_err()
    );
    assert!(
        strict::parse_fragment(r#"{"backend_dir":"a","backend_dir":"b"}"#, "恢复参数").is_err()
    );
    let window = reads::Window::begin().unwrap();
    assert!(window.finish(Ok(())).is_ok());
}

#[test]
fn source_read_on_an_unbound_worker_thread_cannot_claim_complete_protection() {
    use crate::recovery::clone::native::source;
    let directory = fixture();
    let file = directory.path().join("input.json");
    std::fs::write(&file, b"{\"value\":1}").unwrap();
    let digest = digest::file(&file).unwrap();
    let inventory = source::Inventory {
        source: source::SourceBinding {
            snapshot: source::SourceSnapshot {
                head: "a".repeat(40),
                patch_sha256: "b".repeat(64),
                files: vec![],
                clean: true,
            },
            worktree_fingerprint: format!("sha256:{}", "c".repeat(64)),
        },
        files: vec![source::FileRecord {
            path: "input.json".into(),
            sha256: digest.sha256,
        }],
        guard: source::SourceGuard {
            head: "a".repeat(40),
            index_sha256: "d".repeat(64),
            modes_sha256: "e".repeat(64),
        },
    };
    let window = reads::Window::begin().unwrap();
    assert!(
        std::thread::spawn(move || strict::read(&file).is_ok())
            .join()
            .unwrap()
    );
    let error = reads::source(directory.path(), &inventory).unwrap_err();
    assert!(error.to_string().contains("并行来源读取"));
    assert!(window.finish::<()>(Err(error)).is_err());
}

#[test]
fn execution_verification_retains_authority_protection_for_each_consumed_source_root() {
    use crate::recovery::fixture_control::source_pair::execution::{self, tests::Fixture};
    let fixture = Fixture::new();
    let publication = fixture.publication();
    let binding = fixture.binding();
    // 这些源码仅由完整来源采集消费，不借助契约资产专读取得保护。
    for file in [
        fixture.backend.join("src/lib.rs"),
        fixture.frontend.join("src/business.txt"),
        fixture.output.join("backend/src/lib.rs"),
        fixture.output.join("frontend/src/business.txt"),
    ] {
        let before = std::fs::read(&file).unwrap();
        let window = reads::Window::begin().unwrap();
        assert_eq!(
            execution::verify(&fixture.backend, &binding).unwrap(),
            publication
        );
        match std::fs::write(&file, "// 权威窗口内实际消费的来源漂移\n") {
            Ok(()) => assert!(
                window.finish(Ok(())).is_err(),
                "执行核验遗漏已消费来源的后验保护：{}",
                file.display()
            ),
            Err(error) => {
                assert!(
                    cfg!(windows)
                        && (error.kind() == std::io::ErrorKind::PermissionDenied
                            || matches!(error.raw_os_error(), Some(32 | 33))),
                    "来源写入出现非预期错误：{error}; kind={:?}, raw_os_error={:?}",
                    error.kind(),
                    error.raw_os_error()
                );
                assert_eq!(std::fs::read(&file).unwrap(), before);
                window.finish(Ok(())).unwrap();
            }
        }
        drop(window);
        std::fs::write(&file, &before).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), before);
    }
    assert_eq!(
        execution::verify(&fixture.backend, &binding).unwrap(),
        publication
    );
}

fn write_lineage_receipt(
    backend: &std::path::Path,
    path: &std::path::Path,
    value: &Value,
) -> Value {
    use crate::recovery::inputs::document;

    strict::write_new(path, value).unwrap();
    document::binding(&document::read(path, backend, "测试血缘收据").unwrap())
}

fn lineage_chain(
    backend: &std::path::Path,
    root: &std::path::Path,
    recovery: Option<Value>,
) -> Value {
    use crate::recovery::clone::native::seed::generation::coordinator::receipts::{
        RESULT_FIELDS, START_FIELDS,
    };

    let empty = |fields: &[&str]| {
        Value::Object(
            fields
                .iter()
                .map(|&field| (field.to_owned(), Value::Null))
                .collect(),
        )
    };
    let lineage = write_lineage_receipt(
        backend,
        &root.join("lineage.json"),
        &json!({"source":"origin-a"}),
    );
    let mut start = empty(START_FIELDS);
    start["status"] = json!("seed_source_generation_running");
    start["dataset_lineage"] = lineage.clone();
    let start = write_lineage_receipt(backend, &root.join("start.json"), &start);
    let mut recorder = json!({
        "format_version":3, "kind":"restore-source-runtime", "status":"source_runtime_verified",
        "source_generation":start, "dataset_lineage":lineage,
        "before":null, "after":null, "producer":null, "verification":null,
        "login_audit":null, "cache_cleanup":null, "evidence":null, "write_effects":null,
        "started_at":null, "verified_at":null, "remote_writes":0, "restore_qualified":false
    });
    if let Some(recovery) = recovery {
        recorder["recovery"] = recovery;
    }
    let recorder = write_lineage_receipt(backend, &root.join("source-runtime.json"), &recorder);
    let mut stop = empty(RESULT_FIELDS);
    stop["status"] = json!("seed_source_generation_published");
    stop["start"] = start;
    stop["source_runtime"] = recorder;
    stop["dataset_lineage"] = lineage;
    let stop = write_lineage_receipt(backend, &root.join("stop.json"), &stop);
    json!({"source_generation":stop, "source_export":{"source_generation":stop}})
}

fn extract_lineage(backend: &std::path::Path, backup: &Value) -> Result<Value> {
    let window = reads::Window::begin()?;
    let mut inputs = inputs::Inputs::new(backend);
    let result = lineage::extract(&mut inputs, backup);
    window.finish(inputs.finish(result))
}

fn lineage_directory_bytes(
    root: &std::path::Path,
) -> std::collections::BTreeMap<std::ffi::OsString, Vec<u8>> {
    std::fs::read_dir(root)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (entry.file_name(), std::fs::read(entry.path()).unwrap())
        })
        .collect()
}

#[test]
fn lineage_returns_only_bound_stop_and_lineage_without_writes() {
    use crate::recovery::inputs::document;

    let backend = crate::workspace::root_dir();
    for recovered in [false, true] {
        let directory = fixture();
        let root = directory.path();
        let recovery = recovered.then(|| {
            write_lineage_receipt(
                &backend,
                &root.join("recovery.json"),
                &json!({"recovered":true}),
            )
        });
        let backup = lineage_chain(&backend, root, recovery);
        let lineage = document::read(&root.join("lineage.json"), &backend, "测试数据血缘").unwrap();
        let before = lineage_directory_bytes(root);
        assert_eq!(
            extract_lineage(&backend, &backup).unwrap(),
            json!({"source_generation":backup["source_generation"], "dataset_lineage":document::binding(&lineage)})
        );
        assert_eq!(lineage_directory_bytes(root), before);
    }
}

#[test]
fn lineage_refuses_cross_generation_receipts_and_a_different_export_stop() {
    let backend = crate::workspace::root_dir();
    let directory = fixture();
    let other = fixture();
    let root = directory.path();
    let backup = lineage_chain(&backend, root, None);
    let other_backup = lineage_chain(&backend, other.path(), None);
    let stop = strict::read(&root.join("stop.json")).unwrap();
    let other_stop = strict::read(&other.path().join("stop.json")).unwrap();
    for field in ["start", "source_runtime", "dataset_lineage"] {
        let mut modified = stop.clone();
        modified[field] = other_stop[field].clone();
        let replaced = write_lineage_receipt(
            &backend,
            &root.join(format!("other-{field}.json")),
            &modified,
        );
        let altered =
            json!({"source_generation":replaced, "source_export":{"source_generation":replaced}});
        let error = extract_lineage(&backend, &altered).unwrap_err().to_string();
        assert!(error.contains("不是同一不可变来源"), "{field}: {error}");
    }
    let mut wrong_export = backup;
    wrong_export["source_export"]["source_generation"] = other_backup["source_generation"].clone();
    let error = extract_lineage(&backend, &wrong_export)
        .unwrap_err()
        .to_string();
    assert!(error.contains("不是同一不可变来源"), "{error}");
}

#[test]
fn lineage_refuses_extra_receipt_fields_and_unpublished_stop() {
    let backend = crate::workspace::root_dir();
    let directory = fixture();
    let root = directory.path();
    lineage_chain(&backend, root, None);
    let stop = strict::read(&root.join("stop.json")).unwrap();
    let reject_stop = |name: &str, modified: &Value, reason: &str| {
        let binding = write_lineage_receipt(&backend, &root.join(name), modified);
        let backup =
            json!({"source_generation":binding, "source_export":{"source_generation":binding}});
        let error = extract_lineage(&backend, &backup).unwrap_err().to_string();
        assert!(error.contains(reason), "{name}: {error}");
    };
    let mut unknown_stop = stop.clone();
    unknown_stop["unexpected"] = json!(true);
    reject_stop("extra-stop.json", &unknown_stop, "备份停止代次字段");

    let mut running = stop.clone();
    running["status"] = json!("seed_source_generation_running");
    reject_stop("unpublished-stop.json", &running, "不是同一不可变来源");

    for (file, field, reason) in [
        ("start.json", "start", "备份启动代次字段"),
        (
            "source-runtime.json",
            "source_runtime",
            "备份来源运行验收字段",
        ),
    ] {
        let mut receipt = strict::read(&root.join(file)).unwrap();
        receipt["unexpected"] = json!(true);
        let mut rebound = stop.clone();
        rebound[field] =
            write_lineage_receipt(&backend, &root.join(format!("extra-{file}")), &receipt);
        reject_stop(&format!("stop-with-extra-{file}"), &rebound, reason);
    }
}

#[test]
fn lineage_refuses_malformed_or_stale_recovery_descriptors() {
    let backend = crate::workspace::root_dir();
    let source = fixture();
    let original = write_lineage_receipt(
        &backend,
        &source.path().join("recovery.json"),
        &json!({"recovered":true}),
    );
    let mut extra = original.clone();
    extra["unexpected"] = json!(true);
    let mut stale = original;
    stale["sha256"] = json!("0".repeat(64));
    for (descriptor, expected) in [
        (Value::Null, "恢复静态权威绑定"),
        (json!({}), "恢复静态权威绑定"),
        (extra, "恢复静态权威绑定"),
        (stale, "与登记描述不一致"),
    ] {
        let directory = fixture();
        let backup = lineage_chain(&backend, directory.path(), Some(descriptor));
        let error = extract_lineage(&backend, &backup).unwrap_err().to_string();
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn lineage_refuses_modified_bytes_after_binding_even_with_the_same_length() {
    let backend = crate::workspace::root_dir();
    let directory = fixture();
    let root = directory.path();
    let backup = lineage_chain(&backend, root, None);
    let path = root.join("lineage.json");
    let original = std::fs::read_to_string(&path).unwrap();
    let changed = original.replace("origin-a", "origin-b");
    assert_ne!(original, changed);
    assert_eq!(original.len(), changed.len());
    std::fs::write(path, changed).unwrap();
    let before = lineage_directory_bytes(root);
    let error = extract_lineage(&backend, &backup).unwrap_err().to_string();
    assert!(error.contains("与登记描述不一致"), "{error}");
    assert_eq!(lineage_directory_bytes(root), before);
}

// 启动登记选取的参考计划可以与共享备份的原参考计划属于不同侧。
#[test]
fn registration_reference_uses_the_launch_arm_and_rejects_binding_drift() {
    use crate::recovery::inputs::document;

    let backend = crate::workspace::root_dir();
    let directory = fixture();
    let root = directory.path();
    let bind = |name: &str, value: &Value| write_lineage_receipt(&backend, &root.join(name), value);
    let plan = |side: &str| {
        json!({"format_version":1,"id":"shared-backup","target_side":side,
            "target":{"scope_id":format!("fixture-{side}")}})
    };
    let original = bind("backup-reference.json", &plan("base"));
    let backup = bind(
        "backup.json",
        &json!({"format_version":2,"kind":"restore-reference-backup","reference_plan":original}),
    );
    let selected = plan("candidate");
    let selected_binding = bind("selected-reference.json", &selected);
    let target_binding = bind(
        "target.json",
        &json!({"format_version":1,"kind":"restore-reference-target-plan","target_side":"candidate",
            "reference_plan_sha256":document::hash(&selected).unwrap(),"backup_receipt":backup}),
    );
    let registration_path = root.join("registration.json");
    let sidecar = configuration_registration::sidecar(&registration_path).unwrap();
    std::fs::create_dir(&sidecar).unwrap();
    let environment = write_lineage_receipt(
        &backend,
        &sidecar.join("environment.json"),
        &json!({"scope_id":"fixture-candidate","metrics_bearer_token":"fixture-token",
            "api_config_dir":sidecar.join("roles/config"),"worker_config_dir":sidecar.join("roles/worker-config")}),
    );
    let registration = bind(
        "registration.json",
        &json!({"format_version":2,"kind":"restore-runtime-registration","reference_plan":selected_binding,
            "target_plan":target_binding,"runtime_environment":environment,"remote_writes":0,
            "observation":{"ports_idle":["api","worker","frontend"],"runtime":{"exists":false,"entries":[],
                "anchor":{"path":root,"device":1,"inode":2,"created_ns":3}}}}),
    );
    let launch = json!({"request":{"registration":{"registration":registration,"target_plan":target_binding}}});
    let descriptor_mismatch = "恢复静态权威输入与登记描述不一致";

    {
        let mut inputs = inputs::Inputs::new(&backend);
        let target = inputs.bound(&target_binding).unwrap();
        let backup = inputs.bound(&target.value["backup_receipt"]).unwrap();
        let original = inputs.bound(&backup.value["reference_plan"]).unwrap();
        let reference = registration::reference(&mut inputs, &launch, &target).unwrap();
        assert_eq!(document::binding(&reference), selected_binding);
        assert_eq!(reference.value, selected);
        assert_eq!(original.value["target_side"], "base");
        assert_ne!(reference.path, original.path);

        for (field, reason) in [
            ("target_plan", "恢复代次未绑定显式登记和目标计划"),
            ("registration", descriptor_mismatch),
        ] {
            let mut wrong_launch = launch.clone();
            wrong_launch["request"]["registration"][field]["sha256"] = json!("0".repeat(64));
            let error = registration::reference(&mut inputs, &wrong_launch, &target).unwrap_err();
            assert!(error.to_string().contains(reason), "{field}: {error}");
        }
        inputs.finish(Ok(())).unwrap();
    }

    let selected_path = root.join("selected-reference.json");
    let original = std::fs::read_to_string(&selected_path).unwrap();
    let tampered = original.replace("fixture-candidate", "fixture-candidatz");
    assert_ne!(original, tampered);
    assert_eq!(original.len(), tampered.len());
    std::fs::write(selected_path, tampered).unwrap();

    let mut inputs = inputs::Inputs::new(&backend);
    let target = inputs.bound(&target_binding).unwrap();
    let error = registration::reference(&mut inputs, &launch, &target).unwrap_err();
    assert!(error.to_string().contains(descriptor_mismatch), "{error}");
}

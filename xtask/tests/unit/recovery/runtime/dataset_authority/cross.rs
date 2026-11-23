use super::*;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fs, path::Path};

// 产品 RestorePlan、RestoreDatabase 声明顺序的 JSON，包含微秒与数据库映射。
const PRODUCT_PLAN_JSON: &str = concat!(
    r#"{"id":"restore-arm","backup_id":"formal-backup","scope_id":"arm-b1","fault_at":"2026-11-09T03:00:00.123456Z","#,
    r#""databases":[{"source_key":"shared-control","target_key":"shared-control","#,
    r#""server_uuid":"11111111-1111-4111-8111-111111111111","database":"ryframe_b1_control"}],"#,
    r#""object_endpoint":"http://127.0.0.1:19000","object_prefix":"arm-b1/","#,
    r#""api_ready_url":"http://127.0.0.1:17001/readyz","worker_ready_url":"http://127.0.0.1:17002/readyz","#,
    r#""frontend_sha":"dddddddddddddddddddddddddddddddddddddddd"}"#,
);

struct Fixture {
    bindings: Value,
    target: Value,
    reference: Value,
    manifest: Value,
}

impl Fixture {
    fn new() -> Self {
        let product: Value = serde_json::from_str(PRODUCT_PLAN_JSON).unwrap();
        let plan_hash = hex::encode(Sha256::digest(PRODUCT_PLAN_JSON.as_bytes()));
        let product_plan_sha256 = document::hash(&product).unwrap();
        let manifest = json!({
            "id":"formal-backup", "source_sha":"a".repeat(40),
            "captured_at":"2026-11-09T10:30:00+08:00", "objects":[],
        });
        Self {
            bindings: json!({
                "record":{
                    "plan":product, "plan_hash":plan_hash, "status":"data_verified",
                    "started_at":"2026-11-09T11:00:00+08:00",
                    "data_verified_at":"2026-11-09T11:12:00+08:00",
                    "completed_at":null, "recovered_at":null, "failure":null,
                },
                "manifest":manifest,
            }),
            target: json!({
                "product_plan":product, "product_plan_sha256":product_plan_sha256,
                "product_execution":{
                    "backend_product_sha":"b".repeat(40), "backend_execution_sha":"b".repeat(40),
                    "frontend_sha":"d".repeat(40), "adapter":null,
                },
            }),
            reference: json!({"target":{"frontend_url":"http://127.0.0.1:17003"}}),
            manifest,
        }
    }

    fn authority(&self) -> Result<Value> {
        start_authority(
            &self.bindings,
            &self.target,
            &self.reference,
            &self.manifest,
        )
    }

    fn runtime_receipt(&mut self) -> (tempfile::TempDir, Value) {
        let backend = crate::workspace::root_dir();
        let local = backend.join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("runtime-cross-")
            .tempdir_in(local)
            .unwrap();
        let root = directory.path();
        let generation = root.join("g0001");
        let frontend_root = root.join("frontend");
        for path in [&generation, &frontend_root] {
            fs::create_dir(path).unwrap();
        }
        let write = |path: &Path, value: &Value| {
            strict::write_new(path, value).unwrap();
            document::read(path, &backend, "恢复运行测试收据").unwrap()
        };
        let bindings = write(&root.join("bindings.json"), &self.bindings);
        let backend_build = write(
            &root.join("backend-build.json"),
            &json!({"format_version":2, "kind":"restore-backend-build",
                "artifacts":{"api":{"executable":root.join("api.exe")},
                    "worker":{"executable":root.join("worker.exe")}}}),
        );
        let frontend_build = write(
            &frontend_root.join("restore-build.json"),
            &json!({"format_version":3, "kind":"restore-frontend-build"}),
        );
        self.target["product_execution"]["roots"] =
            json!({"source_backend":root, "execution_backend":root, "frontend":frontend_root});
        self.target["product_execution"]["builds"] = json!({
            "backend": document::binding(&backend_build),
            "frontend": document::binding(&frontend_build),
        });
        let authority = self.authority().unwrap();
        let mut paths = json!({});
        let mut digests = json!({});
        for (name, doc) in [
            ("bindings", &bindings),
            ("backend_build", &backend_build),
            ("frontend_build", &frontend_build),
        ] {
            paths[name] = json!(doc.path);
            digests[name] = json!(doc.sha256());
        }
        let mut processes = json!({});
        let mut launch_processes = json!({});
        for (role, pid) in [("api", 11), ("worker", 12), ("frontend", 13)] {
            let identity =
                json!({"pid":pid, "started":"1000", "executable":root.join(format!("{role}.exe"))});
            let process = write(
                &generation.join(format!("{role}.json")),
                &json!({"format_version":1, "role":role, "scope_id":authority["scope_id"], "identity":identity}),
            );
            let mut receipt = document::binding(&process);
            receipt["identity"] = identity.clone();
            launch_processes[role] = json!({"process_receipt":receipt});
            processes[role] = json!({"receipt_path":process.path,
                "receipt_sha256":process.sha256(), "identity":identity});
        }
        let launch = write(
            &generation.join("runtime-launch.json"),
            &json!({"format_version":1, "kind":"restore-runtime-launch", "generation":1,
                "runtime_directory":generation, "processes":launch_processes,
                "request":{"format_version":2, "kind":"restore-runtime-launch-request",
                    "authority":authority, "paths":paths, "digests":digests,
                    "roots":{"backend_product":root, "backend_execution":root, "frontend":frontend_root},
                    "artifacts":backend_build.value["artifacts"]}}),
        );
        paths["launch"] = json!(launch.path);
        paths["runtime_dir"] = json!(generation);
        paths["backend_product_root"] = json!(root);
        paths["backend_execution_root"] = json!(root);
        paths["frontend_root"] = json!(frontend_root);
        digests["launch"] = json!(launch.sha256());
        let runtime = json!({"format_version":3, "kind":"restore-runtime",
            "paths":paths, "digests":digests, "processes":processes,
            "restore":{"id":authority["restore_id"], "backup_id":authority["backup_id"],
                "scope_id":authority["scope_id"], "plan_hash":authority["plan_hash"],
                "data_verified_at":authority["data_verified_at"]},
            "source":{"backup_source_sha":authority["backup_source_sha"],
                "backend_product_sha":authority["backend_product_sha"],
                "backend_execution_sha":authority["backend_execution_sha"],
                "frontend_sha":authority["frontend_sha"], "backend_adapter_contract":null},
            "endpoints":{"api":authority["api_endpoint"], "worker":authority["worker_endpoint"],
                "frontend":authority["frontend_endpoint"]},
            "backend":backend_build.value, "frontend":frontend_build.value});
        (directory, runtime)
    }
}

#[test]
fn start_authority_preserves_the_complete_v2_projection_for_ordinary_and_b0() {
    let mut fixture = Fixture::new();
    let mut expected = json!({
        "format_version":2, "kind":"restore-runtime-authority",
        "restore_id":"restore-arm", "backup_id":"formal-backup", "scope_id":"arm-b1",
        "plan_hash":fixture.bindings["record"]["plan_hash"],
        "data_verified_at":"2026-11-09T11:12:00+08:00",
        "backup_source_sha":"a".repeat(40),
        "backend_product_sha":"b".repeat(40), "backend_execution_sha":"b".repeat(40),
        "frontend_sha":"d".repeat(40), "backend_adapter_contract":null,
        "api_endpoint":"http://127.0.0.1:17001/readyz",
        "worker_endpoint":"http://127.0.0.1:17002/readyz",
        "frontend_endpoint":"http://127.0.0.1:17003",
    });
    assert_ne!(expected["plan_hash"], fixture.target["product_plan_sha256"]);
    assert_eq!(fixture.authority().unwrap(), expected);

    let execution = &mut fixture.target["product_execution"];
    execution["backend_execution_sha"] = json!("c".repeat(40));
    execution["adapter"] = json!({"contract":"legacy-stable-readiness-b0-v1"});
    expected["backend_execution_sha"] = json!("c".repeat(40));
    expected["backend_adapter_contract"] = json!("legacy-stable-readiness-b0-v1");
    assert_eq!(fixture.authority().unwrap(), expected);
}

#[test]
fn start_authority_validates_the_product_and_target_document_digests_separately() {
    let mut fixture = Fixture::new();
    fixture.bindings["record"]["plan_hash"] = fixture.target["product_plan_sha256"].clone();
    assert!(
        fixture
            .authority()
            .unwrap_err()
            .to_string()
            .contains("恢复记录计划摘要与已核验产品计划不同")
    );

    let mut fixture = Fixture::new();
    fixture.target["product_plan_sha256"] = fixture.bindings["record"]["plan_hash"].clone();
    assert!(
        fixture
            .authority()
            .unwrap_err()
            .to_string()
            .contains("目标计划的产品文档摘要不同")
    );
}

#[test]
fn start_authority_rejects_changed_plan_manifest_status_backup_id_and_plan_digest() {
    let mut fixture = Fixture::new();
    fixture.bindings["record"]["plan"]["object_prefix"] = json!("different/");
    assert!(
        fixture
            .authority()
            .unwrap_err()
            .to_string()
            .contains("唯一备份")
    );

    let mut fixture = Fixture::new();
    fixture.bindings["manifest"]["captured_at"] = json!("2026-11-09T10:35:00+08:00");
    assert!(
        fixture
            .authority()
            .unwrap_err()
            .to_string()
            .contains("唯一备份")
    );

    let mut fixture = Fixture::new();
    fixture.bindings["record"]["status"] = json!("started");
    assert!(
        fixture
            .authority()
            .unwrap_err()
            .to_string()
            .contains("唯一备份")
    );

    let mut fixture = Fixture::new();
    fixture.manifest["id"] = json!("another-backup");
    fixture.bindings["manifest"] = fixture.manifest.clone();
    assert!(
        fixture
            .authority()
            .unwrap_err()
            .to_string()
            .contains("唯一备份")
    );

    let mut fixture = Fixture::new();
    fixture.bindings["record"]["plan_hash"] = json!("e".repeat(64));
    assert!(
        fixture
            .authority()
            .unwrap_err()
            .to_string()
            .contains("计划摘要与已核验产品计划不同")
    );
}

#[test]
fn start_authority_validates_time_frontend_adapter_and_distinct_ports() {
    let mut fixture = Fixture::new();
    fixture.bindings["record"]["data_verified_at"] = json!("2026-11-09T11:12:00");
    assert!(
        fixture
            .authority()
            .unwrap_err()
            .to_string()
            .contains("缺少时区")
    );

    let mut fixture = Fixture::new();
    fixture.target["product_execution"]["frontend_sha"] = json!("e".repeat(40));
    assert!(
        fixture
            .authority()
            .unwrap_err()
            .to_string()
            .contains("前端执行源码")
    );

    let mut fixture = Fixture::new();
    fixture.target["product_execution"]["backend_execution_sha"] = json!("c".repeat(40));
    assert!(
        fixture
            .authority()
            .unwrap_err()
            .to_string()
            .contains("适配关系无效")
    );

    let mut fixture = Fixture::new();
    fixture.reference["target"]["frontend_url"] = json!("http://127.0.0.1:17002");
    assert!(
        fixture
            .authority()
            .unwrap_err()
            .to_string()
            .contains("互异显式端口")
    );
}

#[test]
fn read_checks_all_document_digests_and_canonical_generation_path() {
    let mut fixture = Fixture::new();
    let (directory, runtime) = fixture.runtime_receipt();
    let backend = crate::workspace::root_dir();
    let read_runtime = |value: &Value| {
        let mut inputs = Inputs::new(&backend);
        let result = read(&mut inputs, value);
        inputs.finish(result)
    };
    let documents = read_runtime(&runtime).unwrap();
    for (name, original) in [
        ("bindings", &documents.bindings),
        ("backend_build", &documents.backend),
        ("frontend_build", &documents.frontend),
    ] {
        assert_eq!(json!(original.sha256()), runtime["digests"][name]);
        let mut changed = original.raw.clone();
        changed.push(b' ');
        fs::write(&original.path, &changed).unwrap();
        let error = read_runtime(&runtime).err().unwrap().to_string();
        assert!(error.contains("恢复运行输入摘要漂移"), "{name}: {error}");
        assert_eq!(fs::read(&original.path).unwrap(), changed);
        fs::write(&original.path, &original.raw).unwrap();
        read_runtime(&runtime).unwrap();
    }
    let launch = documents.launch;
    assert_eq!(json!(launch.sha256()), runtime["digests"]["launch"]);
    assert_eq!(
        launch.path.canonicalize().unwrap(),
        directory
            .path()
            .join("g0001/runtime-launch.json")
            .canonicalize()
            .unwrap()
    );

    let other_generation = directory.path().join("g0002");
    fs::create_dir(&other_generation).unwrap();
    let other_launch = other_generation.join("runtime-launch.json");
    fs::copy(&launch.path, &other_launch).unwrap();
    assert_eq!(fs::read(&other_launch).unwrap(), launch.raw);
    let mut misplaced = runtime.clone();
    misplaced["paths"]["launch"] = json!(other_launch);
    let error = read_runtime(&misplaced).err().unwrap().to_string();
    assert!(
        error.contains("恢复运行启动收据路径不属于同代目录"),
        "{error}"
    );

    let mut changed = launch.value;
    changed["generation"] = json!(2);
    let changed = document::canonical_bytes(&changed).unwrap();
    assert_eq!(changed.len(), launch.raw.len());
    assert_ne!(changed, launch.raw);
    fs::write(&launch.path, changed).unwrap();
    let error = read_runtime(&runtime).err().unwrap().to_string();
    assert!(error.contains("恢复运行输入摘要漂移"), "{error}");
}

#[test]
fn verify_binds_projected_authority_builds_and_processes() {
    use super::super::schema;

    let mut fixture = Fixture::new();
    let (directory, runtime) = fixture.runtime_receipt();
    let generation = directory.path().join("g0001");
    let frontend = directory.path().join("frontend");
    let members = || {
        [directory.path(), &generation, &frontend]
            .into_iter()
            .flat_map(|path| fs::read_dir(path).unwrap())
            .map(|entry| entry.unwrap().path())
            .collect::<BTreeSet<_>>()
    };
    let before = members();
    let backend = crate::workspace::root_dir();
    let mut inputs = Inputs::new(&backend);
    schema::runtime(&runtime).unwrap();
    let documents = read(&mut inputs, &runtime).unwrap();
    let verify_runtime = |runtime: &Value, target: &Value| {
        verify(
            runtime,
            target,
            &fixture.reference,
            &fixture.manifest,
            &documents,
        )
    };
    assert_eq!(
        verify_runtime(&runtime, &fixture.target).unwrap(),
        fixture.authority().unwrap()
    );

    let mut changed_process = runtime.clone();
    changed_process["processes"]["worker"]["identity"]["started"] = json!("1001");
    schema::runtime(&changed_process).unwrap();
    let error = verify_runtime(&changed_process, &fixture.target)
        .unwrap_err()
        .to_string();
    assert!(error.contains("恢复运行进程与 launch 绑定不同"), "{error}");

    let mut changed_target = fixture.target.clone();
    let build = &mut changed_target["product_execution"]["builds"]["backend"];
    build["sha256"] = json!("e".repeat(64));
    document::binding_shape(build, "替换后的构建绑定").unwrap();
    let error = verify_runtime(&runtime, &changed_target)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("恢复构建未绑定目标计划的产品执行来源"),
        "{error}"
    );
    inputs.finish(Ok(())).unwrap();
    assert_eq!(members(), before);
}

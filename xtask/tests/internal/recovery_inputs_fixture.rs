use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use serde_json::{Value, json};

use super::cli::{
    BindingsInputOptions, ProductInputOptions, ReferenceInputOptions, RestoreInputPublication,
    RestoreInputSide,
};
use super::recovery::inputs::{product, reference};
use chrono::{SecondsFormat, Utc};
type Object = serde_json::Map<String, Value>;

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

const KEYS: [(&str, &str, &str); 4] = [
    ("shared-control", "combined", "shared"),
    ("shared", "tenant", "shared"),
    ("dedicated-a", "tenant", "dedicated"),
    ("dedicated-b", "tenant", "dedicated"),
];
const TARGET_NAMES: [&str; 4] = ["shared-control", "shared", "dedicated-a", "dedicated-b"];

struct Base {
    backend: PathBuf,
    root: PathBuf,
    tool: PathBuf,
    defaults: PathBuf,
    work_dir: PathBuf,
}

struct TargetState {
    review: Value,
    environment: Value,
    storage_run: Value,
    registration_binding: Value,
    initialized_files_binding: Value,
    initialized_binding: Value,
    fresh_verify: PathBuf,
}

struct SourceState {
    export: Value,
    generation: Value,
    request: Value,
}

struct ArmState {
    arm_input: PathBuf,
}

fn setup() -> Base {
    let backend = super::workspace::root_dir();
    let root = backend.join(".local-tests").join(format!(
        "recovery-inputs-native-{}-{}",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();
    let tool = root.join("tool.exe");
    fs::write(&tool, b"native recovery tool").unwrap();
    let defaults = root.join("client.cnf");
    fs::write(&defaults, b"[client]\nhost=127.0.0.1\n").unwrap();
    let work_dir = root.join("work").join("reference");
    fs::create_dir_all(work_dir.parent().unwrap()).unwrap();
    Base {
        backend,
        root,
        tool,
        defaults,
        work_dir,
    }
}

fn prepare_target(base: &Base) -> TargetState {
    let review = review(&base.backend, &base.root, &base.defaults, &base.tool);
    let review_path = write_value(&base.root, "review.json", &review);
    let review_binding = review_binding(&review_path, &review);
    let selected = review["scopes"]["base"].clone();
    let target_request = target_request(
        &base.defaults,
        &base.tool,
        &review_binding,
        &selected,
        &base.root,
    );
    let target_request_path = write_value(&base.root, "target-request.json", &target_request);
    let registration = registration_state(base, &binding(&target_request_path), &selected);
    let fresh_verify = fresh_verify(&base.root, &selected, &registration);
    TargetState {
        review,
        environment: registration.environment,
        storage_run: registration.storage_run,
        registration_binding: registration.registration_binding,
        initialized_files_binding: registration.initialized_files_binding,
        initialized_binding: registration.initialized_binding,
        fresh_verify,
    }
}

struct RegistrationState {
    environment: Value,
    storage_run: Value,
    registration_binding: Value,
    initialized_files_binding: Value,
    initialized_binding: Value,
}

fn registration_state(base: &Base, request: &Value, selected: &Value) -> RegistrationState {
    let target_workspace = base.root.join("target-workspace");
    let target_dir = target_workspace.join("target");
    fs::create_dir_all(&target_dir).unwrap();
    let storage_run = base.root.join("storage-run");
    fs::create_dir_all(&storage_run).unwrap();
    let environment_path = write_value(
        &base.root,
        "target-environment.json",
        &json!({"environment": {}}),
    );
    let environment = binding(&environment_path);
    let storage_manifest = write_value(&storage_run, "manifest.json", &json!({"kind": "fixture"}));
    let value = json!({
        "path": storage_run.to_string_lossy(),
        "manifest": binding(&storage_manifest),
        "storage": {"status": "ready"},
        "cache": {"status": "ready"},
    });
    let registration = write_value(
        &target_workspace,
        "registration.json",
        &json!({
            "format_version": 1,
            "kind": "devex-clone-fresh-target-registration",
            "request": request,
            "environment": environment,
            "storage_run": value,
            "target_directory": target_dir.to_string_lossy(),
        }),
    );
    let registration_binding = binding(&registration);
    let prepared = write_value(
        &target_workspace,
        "prepared-files.json",
        &json!({
            "format_version": 1,
            "kind": "devex-clone-fresh-target-prepared-files",
            "registration": registration_binding,
            "predecessor": null,
            "files": [],
        }),
    );
    let initialized_files = write_value(
        &target_workspace,
        "initialized-files.json",
        &json!({
            "format_version": 1,
            "kind": "devex-clone-fresh-target-initialized-files",
            "registration": registration_binding,
            "predecessor": binding(&prepared),
            "files": [],
        }),
    );
    let initialized = write_value(
        &target_dir,
        "initialized.json",
        &json!({
            "inventory": {"observations": observations(selected)},
            "objects": object_ownership(selected),
            "redis": {"owner": "fixture"},
        }),
    );
    RegistrationState {
        environment,
        storage_run: value,
        registration_binding,
        initialized_files_binding: binding(&initialized_files),
        initialized_binding: binding(&initialized),
    }
}

fn fresh_verify(root: &Path, selected: &Value, registration: &RegistrationState) -> PathBuf {
    let target_workspace = root.join("target-workspace");
    let observations = observations(selected);
    let inventory = write_value(
        &target_workspace,
        "inventory.json",
        &json!({
            "format_version": 1,
            "status": "side_inventory_captured",
            "side": "target",
            "scope_id": selected["scope_id"],
            "binding_sha256": "a".repeat(64),
            "keys": TARGET_NAMES,
            "inventories": {},
            "observations": observations,
            "remote_writes": 0,
            "producer_stopped_proven": false,
            "fresh_target_proven": false,
            "target_ready": false,
            "clone_verified": false,
            "restore_qualified": false,
        }),
    );
    write_value(
        &target_workspace,
        "verify.json",
        &json!({
            "status": "fresh_target_reverified",
            "initialized": registration.initialized_binding,
            "inventory": {"receipt": binding(&inventory), "observations": observations},
            "remote_writes": 0,
            "target_ready": false,
            "clone_verified": false,
            "restore_qualified": false,
        }),
    )
}

fn prepare_source(base: &Base) -> SourceState {
    let source_runtime_dir = base.root.join("source-runtime");
    fs::create_dir_all(&source_runtime_dir).unwrap();
    let bindings = source_input_bindings(&base.root, &source_runtime_dir);
    let tool = json!({
        "path": base.tool.to_string_lossy(),
        "sha256": file_sha(&base.tool),
    });
    let request = source_request(
        &source_runtime_dir,
        &base.defaults,
        &tool,
        &bindings.backend_build,
        &bindings.maintenance_build,
        &bindings.runtime,
        &bindings.api,
        &bindings.worker,
        &bindings.producers,
    );
    let request_path = write_value(&base.root, "source-request.json", &request);
    let request_binding = binding(&request_path);
    let registration = write_value(&base.root, "source-registration.json", &json!({"ok": true}));
    let rebind = write_value(&base.root, "source-rebind.json", &json!({"ok": true}));
    let successor = write_value(&base.root, "review-successor.json", &json!({"ok": true}));
    let result = write_value(
        &base.root,
        "source-export-result.json",
        &json!({"status": "published"}),
    );
    let exported = write_value(
        &base.root,
        "source-export.json",
        &json!({"published": true}),
    );
    let generation = write_value(
        &base.root,
        "source-generation.json",
        &json!({"stopped": true}),
    );
    let export = export_identity(
        &binding(&result),
        &binding(&registration),
        &binding(&rebind),
        &binding(&generation),
        &binding(&successor),
        &request_binding,
        &binding(&exported),
    );
    SourceState {
        export,
        generation: binding(&generation),
        request: request_binding,
    }
}

struct SourceInputBindings {
    backend_build: Value,
    maintenance_build: Value,
    runtime: Value,
    api: Value,
    worker: Value,
    producers: Value,
}

fn source_input_bindings(root: &Path, runtime_dir: &Path) -> SourceInputBindings {
    let bind =
        |name: &str, directory: &Path, value: Value| binding(&write_value(directory, name, &value));
    SourceInputBindings {
        backend_build: bind("backend-build.json", root, json!({"kind": "backend"})),
        maintenance_build: bind(
            "maintenance-build.json",
            root,
            json!({"kind": "devex-clone-tool-build"}),
        ),
        runtime: bind(
            "runtime.json",
            runtime_dir,
            json!({
                "backend_root": super::workspace::root_dir(),
                "api_config_dir": runtime_dir.join("config"),
                "api_configuration_sha256": super::recovery_inputs_support::object_configuration(&runtime_dir.join("config"), "source"),
            }),
        ),
        api: bind("api.json", runtime_dir, json!({"role": "api"})),
        worker: bind("worker.json", runtime_dir, json!({"role": "worker"})),
        producers: bind("producers.json", root, json!({"processes": []})),
    }
}

fn prepare_arm(base: &Base, target: &TargetState, source: &SourceState) -> ArmState {
    let request = write_value(
        &base.root,
        "arm-request.json",
        &json!({
            "format_version": 1,
            "kind": "devex-clone-seed-successor-arm-input",
            "source_registration": source.export["source_registration"],
            "id": "arm-base-run",
            "initialized": target.initialized_binding,
            "target_registration": target.registration_binding,
            "target_initialized_files": target.initialized_files_binding,
            "target_environment": target.environment,
            "copy_directory": base.root.join("arm-copy").to_string_lossy(),
            "build_bridges": [],
            "review_successor": source.export["review_successor"],
            "source_export": source.export["export"],
            "source_export_result": source.export["result"],
            "source_generation": source.generation,
        }),
    );
    let source_environment = write_value(
        &base.root,
        "source-environment.json",
        &json!({"environment": {}}),
    );
    let manifest = write_value(
        &base.root,
        "arm-manifest.json",
        &json!({
            "format_version": 1,
            "kind": "devex-clone-run",
            "id": "arm-base-run",
            "source_request": source.request,
            "source_export": source.export["export"],
            "initialized": target.initialized_binding,
            "source_environment": binding(&source_environment),
            "target_environment": target.environment,
            "copy_directory": base.root.join("arm-copy").to_string_lossy(),
            "copy_stage": "seed_to_arm",
            "build_bridges": [],
            "source_registration": source.export["source_registration"],
            "target_registration": target.registration_binding,
            "target_initialized_files": target.initialized_files_binding,
            "target_storage_run": target.storage_run,
            "review_successor": source.export["review_successor"],
            "source_export_result": source.export["result"],
            "source_generation": source.generation,
        }),
    );
    let arm_input = write_value(
        &base.root,
        "arm-result.json",
        &json!({
            "status": "seed_arm_input_published",
            "request": binding(&request),
            "manifest": binding(&manifest),
            "source_registration": source.export["source_registration"],
            "source_request": source.request,
            "source_storage": {"status": "ready"},
            "generation_verified": {"status": "verified"},
            "initialized": target.initialized_binding,
            "target_environment": target.environment,
            "target_registration": target.registration_binding,
            "target_initialized_files": target.initialized_files_binding,
            "target_storage_run": target.storage_run,
            "target_side": "base",
            "remote_writes": 0,
            "outbox_drained": true,
            "restore_qualified": false,
            "review_successor": source.export["review_successor"],
            "source_export": source.export["export"],
            "source_export_result": source.export["result"],
            "source_generation": source.generation,
        }),
    );
    ArmState { arm_input }
}

struct OutputState {
    reference_plan: PathBuf,
    product_plan: PathBuf,
    target_plan: PathBuf,
    backup_receipt: PathBuf,
    comparison_sources: PathBuf,
    record: PathBuf,
    reference_value: Value,
    product_value: Value,
}

fn prepare_outputs(
    base: &Base,
    target: &TargetState,
    source: &SourceState,
    arm: &ArmState,
) -> OutputState {
    let comparison_sources = base.root.join("comparison.json");
    write_comparison(
        &base.backend,
        &base.root,
        &comparison_sources,
        &source.export,
    );
    let options = reference_options(&arm.arm_input, &target.fresh_verify, &base.work_dir, None);
    let reference_value = reference::build(&base.backend, &options).unwrap();
    let reference_plan = write_value(&base.root, "reference.json", &reference_value);
    let backup_receipt = write_backup(
        &base.backend,
        &reference_value,
        &base.work_dir,
        &source.export,
        &source.generation,
    );
    let options = product_options(
        &reference_plan,
        &backup_receipt,
        &comparison_sources,
        &arm.arm_input,
        &target.fresh_verify,
        &base.root,
        None,
    );
    let product_value = product::build(&base.backend, &options).unwrap();
    let product_plan = write_value(&base.root, "product.json", &product_value);
    let target_value = capture_target_plan(
        &base.backend,
        &reference_plan,
        &backup_receipt,
        &comparison_sources,
        &arm.arm_input,
        &target.fresh_verify,
        &product_plan,
    )
    .unwrap();
    let target_plan = write_value(&base.root, "target-plan.json", &target_value);
    let record = write_value(
        &base.root,
        "record.json",
        &json!({
            "plan": product_value,
            "plan_hash": hash(&product_value),
            "status": "data_verified",
            "started_at": now(),
            "data_verified_at": now(),
            "completed_at": null,
            "recovered_at": now(),
            "failure": null,
        }),
    );
    OutputState {
        reference_plan,
        product_plan,
        target_plan,
        backup_receipt,
        comparison_sources,
        record,
        reference_value,
        product_value,
    }
}

pub struct Fixture {
    pub backend: PathBuf,
    pub root: PathBuf,
    pub work_dir: PathBuf,
    pub arm_input: PathBuf,
    pub fresh_verify: PathBuf,
    pub reference_plan: PathBuf,
    pub product_plan: PathBuf,
    pub target_plan: PathBuf,
    pub backup_receipt: PathBuf,
    pub comparison_sources: PathBuf,
    pub record: PathBuf,
    pub output: PathBuf,
    pub reference_value: Value,
    pub product_value: Value,
    pub review: Value,
    pub defaults: PathBuf,
    pub tool: PathBuf,
}

impl Fixture {
    pub fn new() -> Self {
        let base = setup();
        let target = prepare_target(&base);
        let source = prepare_source(&base);
        let arm = prepare_arm(&base, &target, &source);
        let output = prepare_outputs(&base, &target, &source, &arm);
        Self {
            output: base.root.join("published-reference.json"),
            backend: base.backend,
            root: base.root,
            work_dir: base.work_dir,
            arm_input: arm.arm_input,
            fresh_verify: target.fresh_verify,
            reference_plan: output.reference_plan,
            product_plan: output.product_plan,
            target_plan: output.target_plan,
            backup_receipt: output.backup_receipt,
            comparison_sources: output.comparison_sources,
            record: output.record,
            reference_value: output.reference_value,
            product_value: output.product_value,
            review: target.review,
            defaults: base.defaults,
            tool: base.tool,
        }
    }

    pub fn reference_options(&self, publication: Option<PathBuf>) -> ReferenceInputOptions {
        reference_options(
            &self.arm_input,
            &self.fresh_verify,
            &self.work_dir,
            publication.map(|output| RestoreInputPublication { output }),
        )
    }

    pub fn product_options(&self, publication: Option<PathBuf>) -> ProductInputOptions {
        product_options(
            &self.reference_plan,
            &self.backup_receipt,
            &self.comparison_sources,
            &self.arm_input,
            &self.fresh_verify,
            &self.root,
            publication.map(|output| RestoreInputPublication { output }),
        )
    }

    pub fn bindings_options(&self, publication: Option<PathBuf>) -> BindingsInputOptions {
        BindingsInputOptions {
            reference_plan: self.reference_plan.clone(),
            target_plan: self.target_plan.clone(),
            backup_receipt: self.backup_receipt.clone(),
            record: self.record.clone(),
            publication: publication.map(|output| RestoreInputPublication { output }),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.root.exists() {
            fs::remove_dir_all(&self.root).unwrap();
        }
    }
}

fn review(backend: &Path, root: &Path, defaults: &Path, tool: &Path) -> Value {
    let mut scopes = serde_json::Map::new();
    for (scope_index, (scope, port)) in [("seed", 18070), ("base", 18080), ("candidate", 18090)]
        .into_iter()
        .enumerate()
    {
        let databases = KEYS
            .iter()
            .enumerate()
            .map(|(index, (key, _, _))| {
                json!({
                    "key": key,
                    "database": format!("{}_{}", scope, key.replace('-', "_")),
                    "expected_server_uuid": format!(
                        "00000000-0000-0000-0000-{:012x}",
                        scope_index * 10 + index + 1
                    ),
                    "connection_file": defaults.to_string_lossy(),
                })
            })
            .collect::<Vec<_>>();
        scopes.insert(
            scope.into(),
            json!({
                "scope_id": format!("{scope}-a"),
                "runtime_dir": root.join("runtime").join(scope).to_string_lossy(),
                "identity_ledger": root.join("identity").join(scope).to_string_lossy(),
                "api_url": format!("http://127.0.0.1:{port}"),
                "worker_ready_url": format!("http://127.0.0.1:{}/readyz", port + 1),
                "frontend_url": format!("http://127.0.0.1:{}", port + 2),
                "backend_dir": backend.to_string_lossy(),
                "objects": {"endpoint": "http://127.0.0.1:19000", "region": "us-east-1"},
                "redis": {
                    "url": "redis://127.0.0.1:6380/0",
                    "namespace": format!("ryframe:{{{}}}:", "target-a"),
                    "ownership_key": "ryframe:target-a:owner",
                    "ownership_value": "fixture",
                },
                "databases": databases,
            }),
        );
    }
    let tool_binding = json!({"path": tool.to_string_lossy(), "sha256": file_sha(tool)});
    json!({
        "kind": "review-only-perf-resource-plan-with-readonly-preflight",
        "ready_for_execution": true,
        "reference": {
            "source": {"databases": []},
            "protected_target": {"databases": []},
        },
        "scopes": scopes,
        "tools": {
            "mysql": tool_binding,
            "mysqldump": tool_binding,
            "rustfs": tool_binding,
            "redis_server": {
                "distribution": "fixture",
                "resolved_path": tool.to_string_lossy(),
                "sha256": file_sha(tool),
            },
        },
        "services": {
            "rustfs": {
                "api": "http://127.0.0.1:19000",
                "console": "http://127.0.0.1:19001",
                "data_dir": root.join("rustfs").to_string_lossy(),
            },
            "redis": {"directory": root.join("redis").to_string_lossy()},
        },
    })
}

fn target_request(
    defaults: &Path,
    tool: &Path,
    review_binding: &Value,
    selected: &Value,
    root: &Path,
) -> Value {
    let databases = selected["databases"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(index, database)| {
            let (key, kind, mode) = KEYS[index];
            json!({
                "key": key,
                "kind": kind,
                "mode": mode,
                "database": database["database"],
                "server_uuid": database["expected_server_uuid"],
                "defaults_file": defaults.to_string_lossy(),
                "defaults_sha256": file_sha(defaults),
            })
        })
        .collect::<Vec<_>>();
    let tool_binding = json!({"path": tool.to_string_lossy(), "sha256": file_sha(tool)});
    let maintenance = write_value(
        root,
        "target-maintenance.json",
        &json!({"kind": "devex-clone-tool-build"}),
    );
    let maintenance_binding = binding(&maintenance);
    let configuration_sha256 =
        super::recovery_inputs_support::object_configuration(&root.join("config"), "target");
    json!({
        "format_version": 1,
        "kind": "devex-clone-fresh-target",
        "config_dir": root.join("config"),
        "id": "target-request-a",
        "review": review_binding,
        "side": "base",
        "target": {
            "scope_id": selected["scope_id"],
            "s3": {
                "endpoint": selected["objects"]["endpoint"],
                "region": selected["objects"]["region"],
                "access_key": "target-access",
                "secret_key": "target-secret",
            },
            "databases": databases,
        },
        "maintenance_build": maintenance_binding,
        "configuration_sha256": configuration_sha256,
        "tools": {"mysql": tool_binding},
        "storage": {"rustfs": {}, "redis": {}},
        "reset": {},
    })
}

#[allow(clippy::too_many_arguments)]
fn source_request(
    runtime_dir: &Path,
    defaults: &Path,
    tool: &Value,
    backend_build: &Value,
    maintenance_build: &Value,
    runtime: &Value,
    api: &Value,
    worker: &Value,
    producers: &Value,
) -> Value {
    let databases = KEYS
        .iter()
        .enumerate()
        .map(|(index, (key, kind, mode))| {
            json!({
                "key": key,
                "kind": kind,
                "mode": mode,
                "database": format!("source_{}", key.replace('-', "_")),
                "server_uuid": format!("10000000-0000-0000-0000-{:012x}", index + 1),
                "defaults_file": defaults.to_string_lossy(),
                "defaults_sha256": file_sha(defaults),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "format_version": 1,
        "kind": "devex-clone-source-export",
        "id": "source-export-a",
        "source": {
            "scope_id": "source-a",
            "runtime_dir": runtime_dir.to_string_lossy(),
            "api_url": "http://127.0.0.1:18060",
            "s3": {"configuration": crate::recovery::clone::native::s3::app_configuration::configuration_value(
                &super::workspace::root_dir(), &runtime_dir.join("config"), "test").unwrap()},
            "databases": databases,
        },
        "tools": {
            "mysql": tool,
            "mysqldump": tool,
            "node": tool,
        },
        "backend_build": backend_build,
        "maintenance_build": maintenance_build,
        "worktree_fingerprint": format!("sha256:{}", "d".repeat(64)),
        "runtime": runtime,
        "processes": {"api": api, "worker": worker},
        "producers_registry": producers,
        "max_object_bytes": 1024,
    })
}

fn export_identity(
    result: &Value,
    source_registration: &Value,
    source_rebind: &Value,
    source_generation: &Value,
    review_successor: &Value,
    source_request: &Value,
    export: &Value,
) -> Value {
    let mut identity = json!({
        "result": result,
        "origin_attempt": 1,
        "source_registration": source_registration,
        "source_rebind": source_rebind,
        "source_generation": source_generation,
        "review_successor": review_successor,
        "source_request": source_request,
        "export": export,
        "export_sha256": "e".repeat(64),
        "generation_sha256": "f".repeat(64),
        "logical_inventory_sha256": "1".repeat(64),
    });
    identity["identity_sha256"] = Value::String(hash(&identity));
    identity
}

fn write_comparison(backend: &Path, root: &Path, path: &Path, export: &Value) {
    let arm = |name: &str, head: &str, adapter: Value| {
        let bindings = [
            (
                "backend",
                write_value(root, &format!("{name}-backend.json"), &json!({"ok": true})),
            ),
            (
                "frontend",
                write_value(root, &format!("{name}-frontend.json"), &json!({"ok": true})),
            ),
        ]
        .into_iter()
        .map(|(role, path)| (role.to_owned(), binding(&path)))
        .collect::<Object>();
        let inventory = inventory(head);
        let source_root = root.join(name).join("source");
        let execution_root = if name == "b1" {
            source_root.clone()
        } else {
            root.join(name).join("execution")
        };
        json!({
            "roots": {
                "source_backend": source_root.to_string_lossy(),
                "execution_backend": execution_root.to_string_lossy(),
                "frontend": root.join(name).join("frontend").to_string_lossy(),
            },
            "sources": {"backend": inventory, "frontend": inventory},
            "execution_sources": {"backend": inventory, "frontend": inventory},
            "builds": bindings,
            "adapter": adapter,
            "source_export_result_sha256": export["result"]["sha256"],
            "source_export_identity_sha256": export["identity_sha256"],
        })
    };
    let patch = backend.join("xtask/assets/baseline-adapters/stable-readiness-b0-v1.patch");
    let adapter = json!({
        "contract": "legacy-stable-readiness-b0-v1",
        "base_backend_sha": "815c5eafb09d4b493319d255fb7ab88ddd02c8b6",
        "base_frontend_sha": "0087ea2ecf62530d042b9e52f5c950fb34c66d78",
        "reference_adapter_sha": "c05114bcdf5c369cd74087db6317ce3c8f89bee8",
        "adapter_tree": "2ff15e7f34da1749c7eb27a1a025b9d3811b87a7",
        "reconstructed_tree": "2ff15e7f34da1749c7eb27a1a025b9d3811b87a7",
        "adapter_paths": ["xtask/src/cli.rs"],
        "patch": {"path": "xtask/assets/baseline-adapters/stable-readiness-b0-v1.patch",
                  "bytes": fs::metadata(&patch).unwrap().len(), "sha256": file_sha(&patch)},
    });
    write_value(
        root,
        "comparison.json",
        &json!({
            "format_version": 1,
            "kind": "restore-comparison-sources",
            "source_export": export,
            "arms": {"b0": arm("b0", &"b".repeat(40), adapter), "b1": arm("b1", &"c".repeat(40), Value::Null)},
        }),
    );
    debug_assert_eq!(path, root.join("comparison.json"));
}

fn inventory(head: &str) -> Value {
    json!({
        "source": {
            "snapshot": {"head": head, "patch_sha256": "1".repeat(64), "files": [], "clean": true},
            "worktree_fingerprint": format!("sha256:{}", "2".repeat(64)),
        },
        "files": [],
        "guard": {"head": head, "index_sha256": "3".repeat(64), "modes_sha256": "4".repeat(64)},
    })
}

fn write_backup(
    _backend: &Path,
    plan: &Value,
    work_dir: &Path,
    export: &Value,
    source_generation: &Value,
) -> PathBuf {
    fs::create_dir_all(work_dir.join("backup").join("databases")).unwrap();
    let artifacts = write_backup_artifacts(plan, work_dir);
    let captured = Utc::now()
        .checked_sub_signed(chrono::Duration::seconds(30))
        .unwrap()
        .to_rfc3339_opts(SecondsFormat::Micros, true);
    let databases = plan["source"]["databases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|database| {
            json!({
                "key": database["key"],
                "kind": database["kind"],
                "database": database["database"],
                "server_uuid": database["server_uuid"],
                "shared": database["mode"] == "shared",
                "tables": [{"table": "sys_post", "rows": 1, "sha256": "a".repeat(64)}],
            })
        })
        .collect::<Vec<_>>();
    let objects = ["uploads", "avatar", "exports", "imports", "config-packages"]
        .into_iter()
        .map(|bucket| json!({"bucket": bucket, "prefix": format!("{}/", plan["source"]["scope_id"].as_str().unwrap()), "entries": []}))
        .collect::<Vec<_>>();
    let manifest = json!({
        "id": plan["id"],
        "scope_id": plan["source"]["scope_id"],
        "source_sha": "a".repeat(40),
        "captured_at": captured,
        "quiesced_at": captured,
        "databases": databases,
        "objects": objects,
        "completed_at": now(),
        "retention_until": now(),
        "artifacts": artifacts,
    });
    let manifest_path = work_dir.join("backup").join("manifest.json");
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    write_value(
        work_dir,
        "reference-owner.json",
        &json!({"format_version": 1, "id": plan["id"], "plan_sha256": hash(plan)}),
    );
    write_value(
        work_dir,
        "backup.json",
        &json!({
            "command": "backup",
            "plan_sha256": hash(plan),
            "started_at": now(),
            "status": "completed",
            "completed_at": now(),
            "result": {
                "format_version": 2,
                "kind": "restore-reference-backup",
                "reference_plan": plan,
                "manifest": binding(&manifest_path),
                "backup_root": work_dir.join("backup").to_string_lossy(),
                "artifacts": manifest["artifacts"].as_array().unwrap().len(),
                "source_export": export,
                "source_generation": source_generation,
            },
        }),
    )
}

fn write_backup_artifacts(plan: &Value, work_dir: &Path) -> Vec<Value> {
    let mut artifacts = Vec::new();
    for database in plan["source"]["databases"].as_array().unwrap() {
        let key = database["key"].as_str().unwrap();
        let relative = format!("databases/{key}.sql");
        let path = work_dir.join("backup").join(&relative);
        fs::write(&path, format!("dump {key}\n")).unwrap();
        artifacts.push(json!({
            "relative_path": relative,
            "resource": format!("db:{key}"),
            "bytes": fs::metadata(&path).unwrap().len(),
            "sha256": file_sha(&path),
        }));
    }
    for bucket in ["uploads", "avatar", "exports", "imports", "config-packages"] {
        let relative = format!("objects/{bucket}/index.json");
        let path = work_dir.join("backup").join(&relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, format!("{bucket}\n")).unwrap();
        artifacts.push(json!({
            "relative_path": relative,
            "resource": format!("objects:{bucket}"),
            "bytes": fs::metadata(&path).unwrap().len(),
            "sha256": file_sha(&path),
        }));
    }
    artifacts
}

fn reference_options(
    arm_input: &Path,
    fresh_verify: &Path,
    work_dir: &Path,
    publication: Option<RestoreInputPublication>,
) -> ReferenceInputOptions {
    ReferenceInputOptions {
        arm_input: arm_input.to_path_buf(),
        fresh_target_verify: fresh_verify.to_path_buf(),
        side: RestoreInputSide::Base,
        id: "backup-case-a".into(),
        work_dir: work_dir.to_path_buf(),
        publication,
    }
}

fn product_options(
    reference_plan: &Path,
    backup_receipt: &Path,
    comparison_sources: &Path,
    arm_input: &Path,
    fresh_verify: &Path,
    _root: &Path,
    publication: Option<RestoreInputPublication>,
) -> ProductInputOptions {
    ProductInputOptions {
        reference_plan: reference_plan.to_path_buf(),
        backup_receipt: backup_receipt.to_path_buf(),
        comparison_sources: comparison_sources.to_path_buf(),
        arm_input: arm_input.to_path_buf(),
        fresh_target_verify: fresh_verify.to_path_buf(),
        side: RestoreInputSide::Base,
        id: "restore-case-a".into(),
        fault_at: now_seconds(),
        publication,
    }
}

use super::recovery_inputs_support::{
    binding, capture_target_plan, file_sha, hash, now, now_seconds, object_ownership, observations,
    review_binding, write_value,
};

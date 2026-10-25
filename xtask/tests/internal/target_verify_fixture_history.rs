use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::{Fixture, KEYS, SCOPE, bound, database, descriptor, storage, write};
use crate::recovery::clone::native::{
    digest, json as json_io,
    lifecycle::state,
    maintenance, source,
    target_verify::{
        input::{request, request_configuration},
        proof, reset, tree,
    },
};

/// 只构造外部工具替身的协议证据；不执行 Cargo，也不写共享 target。
pub(super) fn maintenance(fixture: &Fixture) {
    let directory = fixture.local.join("maintenance");
    fs::create_dir(&directory).unwrap();
    let target = fixture.backend.join("target").join("ci").join("backend");
    fs::create_dir_all(&target).unwrap();
    let original = target.join("fixture-cargo-artifact.exe");
    fs::copy(std::env::current_exe().unwrap(), &original).unwrap();
    let canonical = portable(&fs::canonicalize(&original).unwrap());
    assert!(canonical.starts_with(portable(&fs::canonicalize(&target).unwrap())));
    let saved = directory.join("fixture-maintenance.exe");
    fs::copy(&canonical, &saved).unwrap();
    assert_eq!(
        digest::file(&canonical).unwrap(),
        digest::file(&saved).unwrap()
    );
    let inventory = source::capture(&fixture.backend).unwrap();
    let mut artifacts = serde_json::Map::new();
    for (role, feature, name) in [
        ("reset", "bin-reset", "ryframe-reset"),
        ("migrate", "bin-migrate", "ryframe-migrate"),
        ("tenant-data", "bin-tenant-data", "ryframe-tenant-data"),
    ] {
        let output = directory.join(format!("{role}.cargo.jsonl"));
        let log = directory.join(format!("{role}.cargo.log"));
        let event = json!({"reason":"compiler-artifact","target":{"name":name,"kind":["bin"]},"manifest_path":fixture.backend.join("crates/ryframe/Cargo.toml"),"executable":canonical});
        fs::write(&output, format!("{event}\n")).unwrap();
        fs::write(&log, []).unwrap();
        let command = vec![
            "cargo".to_owned(),
            "build".into(),
            "--locked".into(),
            "-p".into(),
            "ryframe".into(),
            "--no-default-features".into(),
            "--features".into(),
            feature.into(),
            "--bin".into(),
            name.into(),
            "--jobs".into(),
            "4".into(),
            "--target-dir".into(),
            target.display().to_string(),
            "--message-format=json".into(),
        ];
        let actual = digest::file(&saved).unwrap();
        artifacts.insert(role.into(),json!({"executable":saved,"cargo_executable":canonical,"command":command,"bytes":actual.bytes,"sha256":actual.sha256,
            "cargo_output":log_binding(&output),"cargo_log":log_binding(&log)}));
    }
    let receipt = json!({"format_version":1,"kind":"devex-clone-tool-build","backend_root":fixture.backend,"source":source::binding_value(&inventory.source),
        "source_inventory":source::inventory_value(&inventory),"toolchain":{"rustc":"explicit-fixture-observation","cargo":"explicit-fixture-observation"},
        "target_directory":target,"artifacts":artifacts,"restore_qualified":false,"resources_modified":false});
    write(&directory.join("build.json"), &receipt);
    maintenance::verify_evidence(
        &fixture.backend,
        &directory.join("build.json"),
        Some(&receipt),
    )
    .unwrap();
}

fn log_binding(path: &Path) -> Value {
    let mut value = descriptor(path);
    value["file"] = json!(path.file_name().unwrap().to_str().unwrap());
    value
}

fn portable(path: &Path) -> PathBuf {
    let value = path.to_str().unwrap();
    PathBuf::from(value.strip_prefix(r"\\?\").unwrap_or(value))
}

pub(super) fn publish(fixture: &Fixture, status_registration: bool) {
    write(&fixture.local.join("request.json"), &fixture.request);
    write(&fixture.target.join("request.json"), &fixture.request);
    write(
        &fixture.local.join("target-environment.json"),
        &json!({"environment":fixture.environment}),
    );
    write(
        &fixture.local.join("source-environment.json"),
        &json!({"environment":{}}),
    );
    write(
        &fixture.local.join("source-request.json"),
        &json!({"kind":"explicit-fixture-source-request"}),
    );
    if !status_registration {
        fs::write(fixture.target.join("copy-target.guard"), [0]).unwrap();
    }
    let binding = request::validate(&fixture.backend, &fixture.request).unwrap();
    let mut generation = request_configuration::validate(
        &fixture.backend,
        &fixture.request,
        &binding,
        &fixture.environment,
    )
    .unwrap();
    generation["maintenance"] =
        json_io::read(&fixture.local.join("maintenance/build.json")).unwrap();
    generation["storage"] = fixture.request["storage"].clone();
    generation["operator_declared_controlled_generation_only"] = json!(true);
    generation["external_writers_discovered"] = json!(false);
    write(
        &fixture.target.join("prepare.json"),
        &prepared(fixture, &generation),
    );
    if status_registration {
        super::observations::prepared(fixture);
    }
    lineage(fixture, "prepared", status_registration);
    if status_registration {
        // prepare 已封存后才进入初始化控制窗口；不把初始化 guard 写进 prepare 前像。
        let mut guard = fs::File::create_new(fixture.target.join("copy-target.guard")).unwrap();
        std::io::Write::write_all(&mut guard, &[0]).unwrap();
    }
    write(
        &fixture.target.join("initialize.started.json"),
        &json!({"at":"2026-10-18T00:01:00Z","generation_sha256":digest::plan_hash(&generation).unwrap()}),
    );
    creation(fixture);
    let completed = reset_files(fixture, &generation);
    migrations(fixture, &generation["maintenance"]);
    let inventory = database::initial(fixture, &generation);
    let history = proof::history(&fixture.backend, &fixture.target).unwrap();
    assert_eq!(history.as_object().unwrap().len(), 17);
    let initialized = json!({"format_version":1,"status":"fresh_target_initialized","id":fixture.request["id"],"scope_id":SCOPE,"completed_at":"2026-10-18T00:04:00Z",
        "generation":generation,"prepare_sha256":digest::file(&fixture.target.join("prepare.json")).unwrap().sha256,"reset":completed,"inventory":inventory,
        "objects":database::objects(),"redis":storage::initialized(),"history":history,"controlled_generation_never_started":true,"external_writers_discovered":false,
        "target_ready":false,"clone_verified":false,"restore_qualified":false});
    write(
        &fixture.target.join("initialized-candidate.json"),
        &initialized,
    );
    write(&fixture.target.join("initialized.json"), &initialized);
    lineage(fixture, "initialized", status_registration);
    write(
        &fixture.run.join("manifest.json"),
        &json!({"format_version":1,"kind":"devex-clone-run","id":"target-verify-run",
        "source_request":bound(&fixture.local.join("source-request.json")),"source_export":null,"initialized":bound(&fixture.target.join("initialized.json")),
        "source_environment":bound(&fixture.local.join("source-environment.json")),"target_environment":bound(&fixture.local.join("target-environment.json")),
        "copy_directory":fixture.local.join("copy"),"copy_stage":"source_to_seed","build_bridges":[]}),
    );
}

fn prepared(fixture: &Fixture, generation: &Value) -> Value {
    let databases = fixture.request["target"]["databases"].as_array().unwrap();
    let intents = databases
        .iter()
        .map(
            |db| json!({"key":db["key"],"server_uuid":db["server_uuid"],"database":db["database"]}),
        )
        .collect::<Vec<_>>();
    let absent=databases.iter().map(|db|json!({"server_uuid":db["server_uuid"],"database":db["database"],"exists":false,"empty_checked":false})).collect::<Vec<_>>();
    let objects = ["avatar", "config-packages", "exports", "imports", "uploads"]
        .map(|name| (name.to_owned(), json!({"keys":[]})))
        .into_iter()
        .collect::<serde_json::Map<_, _>>();
    json!({"format_version":1,"status":"fresh_creation_prepared","prepared_at":"2026-10-18T00:00:00Z","request":bound(&fixture.local.join("request.json")),
        "request_sha256":digest::plan_hash(&fixture.request).unwrap(),"generation":generation,"absence":{"databases":absent,"objects":objects,"redis":{"keys":[],"owner":null,"sentinel":null}},
        "fresh_target_initialized":false,"controlled_generation_only":true,"external_writers_discovered":false,"create_intents":intents})
}

fn lineage(fixture: &Fixture, stage: &str, status_registration: bool) {
    let registration = fixture.local.join("registration.json");
    if stage == "prepared" {
        // 唯一登记在第一份阶段快照之前发布，后续快照只引用同一绑定。
        write(
            &registration,
            &registration_value(fixture, status_registration),
        );
    }
    let predecessor = if stage == "initialized" {
        bound(&fixture.local.join("prepared-files.json"))
    } else {
        Value::Null
    };
    write(
        &fixture.local.join(format!("{stage}-files.json")),
        &json!({"format_version":1,"kind":format!("devex-clone-fresh-target-{stage}-files"),
        "registration":bound(&registration),"predecessor":predecessor,"files":tree::snapshot(&fixture.backend,&fixture.target,false).unwrap()}),
    );
}

fn registration_value(fixture: &Fixture, _status_registration: bool) -> Value {
    let storage = fixture.backend.join(".local-tests/status-storage");
    fs::create_dir(&storage).unwrap();
    write(
        &storage.join("manifest.json"),
        &json!({"kind":"status-storage"}),
    );
    state::initialize(&storage).unwrap();
    let private = fixture.local.join("status-private.json");
    write(
        &private,
        &json!({"environment":{"APP_ENV":"test","TEMP":fixture.local,"TMP":fixture.local}}),
    );
    json!({"format_version":1,"kind":"devex-clone-fresh-target-registration",
        "request":bound(&fixture.local.join("request.json")),"environment":bound(&private),
        "storage_run":{"path":storage,"manifest":bound(&storage.join("manifest.json")),"state":bound(&storage.join("state.json"))},
        "target_directory":fixture.target})
}

fn creation(fixture: &Fixture) {
    for db in fixture.request["target"]["databases"].as_array().unwrap() {
        let stage = format!("create-{}", db["key"].as_str().unwrap());
        record(
            fixture,
            &stage,
            "intent",
            json!({"key":db["key"],"server_uuid":db["server_uuid"],"database":db["database"]}),
        );
        record(
            fixture,
            &stage,
            "confirmed",
            json!({"server_uuid":db["server_uuid"],"database":db["database"],"exists":true,"empty_checked":true}),
        );
    }
    record(
        fixture,
        "sentinel",
        "intent",
        json!({"key":fixture.request["reset"]["sentinel_key"]}),
    );
    record(
        fixture,
        "sentinel",
        "confirmed",
        json!({"keys":[],"owner":null,"sentinel":fixture.request["reset"]["sentinel_value"]}),
    );
}

fn record(fixture: &Fixture, stage: &str, kind: &str, value: Value) {
    let mut record = json!({"at":"2026-10-18T00:02:00Z","stage":stage});
    record[if kind == "intent" {
        "resource"
    } else {
        "observed"
    }] = value;
    write(
        &fixture.target.join(format!("{stage}.{kind}.json")),
        &record,
    );
}

fn reset_manifest(fixture: &Fixture, generation: &Value) -> Value {
    let databases=generation["physical"]["databases"].as_array().unwrap().iter().map(|db| {
        let combined=db["key"]=="shared-control";let mut owners=json!({"tenant-data":format!("ryframe-owner:v1:{SCOPE}:tenant-data")});
        if combined {owners["control"]=json!(format!("ryframe-owner:v1:{SCOPE}:control"));}
        json!({"target_keys":[db["key"]],"host":db["host"],"port":db["port"],"database":db["database"],"control_baseline":combined,"tenant_baseline":true,
            "ownership_markers":owners,"connection":{"username":"fixture","tls_mode":db["tls_mode"],"tls_ca_sha256":null,"tls_client_cert_sha256":null,"tls_client_key_ref_sha256":null}})
    }).collect::<Vec<_>>();
    let prefixes=["avatar","config-packages","exports","imports","uploads"].map(|bucket|json!({"bucket":bucket,"prefix":format!("{SCOPE}/"),
        "ownership_marker_key":format!("{SCOPE}/.ryframe-owner"),"ownership_marker":format!("ryframe-owner:v1:{SCOPE}:object-storage:{bucket}")}));
    use sha2::{Digest, Sha256};
    json!({"manifest_version":4,"environment":"test","scope_id":SCOPE,"code_sha":generation["maintenance"]["source"]["snapshot"]["head"],
        "config_sha":fixture.request["configuration_sha256"],"credential_version":"fixture-v1","confirmation_phrase":format!("RESET-RYFRAME-test-{SCOPE}"),
        "legacy_ownership":fixture.request["reset"]["legacy_ownership"],"databases":databases,
        "object_storage":{"backend":"rustfs","endpoint":"http://127.0.0.1:29000","region":"test-region","use_ssl":false,"prefixes":prefixes},
        "redis":{"host":"127.0.0.1","port":26379,"database":0,"tls":false,"namespace":format!("ryframe:{{{SCOPE}}}:"),"ownership_marker_key":format!("ryframe:{{{SCOPE}}}:.ryframe-owner"),
            "ownership_marker":format!("ryframe-owner:v1:{SCOPE}:redis"),"outside_sentinel_key_sha256":hex::encode(Sha256::digest(fixture.request["reset"]["sentinel_key"].as_str().unwrap().as_bytes()))}})
}

fn reset_files(fixture: &Fixture, generation: &Value) -> Value {
    let manifest = reset_manifest(fixture, generation);
    let hash = digest::plan_hash(&manifest).unwrap();
    let plan = json!({"manifest":manifest,"plan_hash":hash});
    write(&fixture.target.join("reset-plan.json"), &plan);
    write(&fixture.target.join("reset-plan-confirm.json"), &plan);
    fs::create_dir(fixture.target.join("reset-state")).unwrap();
    let phases=["preflight","object_storage","redis","databases","control_baseline","tenant_baselines","verification","release"].map(|phase|(phase.to_owned(),
        json!({"status":"complete","started_at":"2026-10-18T00:02:00Z","completed_at":"2026-10-18T00:03:00Z"}))).into_iter().collect::<serde_json::Map<_,_>>();
    let resources = reset_resources(&manifest);
    let mut ledger = json!({"ledger_version":4,"updated_at":"2026-10-18T00:03:00Z","plan_hash":hash,"phases":phases,"resources":resources});
    for field in [
        "environment",
        "scope_id",
        "code_sha",
        "config_sha",
        "credential_version",
    ] {
        ledger[field] = manifest[field].clone();
    }
    let mut report = ledger.clone();
    report.as_object_mut().unwrap().remove("ledger_version");
    report.as_object_mut().unwrap().remove("updated_at");
    report["report_version"] = json!(2);
    report["status"] = json!("completed");
    report["failed_phase"] = Value::Null;
    report["completed_at"] = json!("2026-10-18T00:03:00Z");
    report["databases"] = json!(
        manifest["databases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|db| format!(
                "{}:{}/{}",
                db["host"].as_str().unwrap(),
                db["port"],
                db["database"].as_str().unwrap()
            ))
            .collect::<Vec<_>>()
    );
    report["redis_namespace"] = manifest["redis"]["namespace"].clone();
    report["object_prefixes"] = json!(
        manifest["object_storage"]["prefixes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| format!(
                "{}:{}",
                item["bucket"].as_str().unwrap(),
                item["prefix"].as_str().unwrap()
            ))
            .collect::<Vec<_>>()
    );
    let mut previous = ledger.clone();
    previous["updated_at"] = json!("2026-10-18T00:02:59Z");
    previous["phases"]["release"]["status"] = json!("running");
    previous["phases"]["release"]["completed_at"] = Value::Null;
    let stem = format!("test-{SCOPE}-{hash}");
    let root = fixture.target.join("reset-state");
    write(&root.join(format!("{stem}.report.json")), &report);
    write(&root.join(format!("{stem}.ledger.json")), &ledger);
    write(
        &root.join(format!(".{stem}.ledger.json.previous")),
        &previous,
    );
    let result = reset::completed(&fixture.backend, &fixture.target, &manifest, &hash).unwrap();
    record(fixture, "reset", "intent", json!({"plan_hash":hash}));
    record(fixture, "reset", "confirmed", result.clone());
    reset_commands(fixture, generation, &plan, &result);
    result
}

fn reset_resources(manifest: &Value) -> serde_json::Map<String, Value> {
    let mut resources = manifest["databases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|db| {
            (
                db["database"].as_str().unwrap().to_owned(),
                json!({"status":"complete"}),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    resources.insert(
        "redis".into(),
        json!({"status":"complete","namespace":manifest["redis"]["namespace"]}),
    );
    for prefix in manifest["object_storage"]["prefixes"].as_array().unwrap() {
        resources.insert(
            format!("object:{}", prefix["bucket"].as_str().unwrap()),
            json!({"status":"complete","prefix":prefix}),
        );
    }
    resources
}

fn command_receipt(fixture: &Fixture, stage: &str, command: Vec<String>, stdout: String) {
    write(
        &fixture.target.join(format!(
            "{stage}-{}.command.json",
            uuid::Uuid::new_v4().simple()
        )),
        &json!({"command":command,"returncode":0,"error_type":null,"stdout":stdout,"stderr":""}),
    );
}

fn reset_commands(fixture: &Fixture, generation: &Value, plan: &Value, result: &Value) {
    let executable = generation["maintenance"]["artifacts"]["reset"]["executable"]
        .as_str()
        .unwrap();
    for stage in ["reset-plan", "reset-plan-confirm"] {
        command_receipt(
            fixture,
            stage,
            vec![
                executable.into(),
                "--config-dir".into(),
                fixture.request["config_dir"].as_str().unwrap().into(),
                "plan".into(),
            ],
            format!(
                "{}\nplan_hash={}\n",
                plan["manifest"],
                plan["plan_hash"].as_str().unwrap()
            ),
        );
    }
    command_receipt(
        fixture,
        "reset-execute",
        vec![
            executable.into(),
            "--config-dir".into(),
            fixture.request["config_dir"].as_str().unwrap().into(),
            "execute".into(),
            "--plan-hash".into(),
            plan["plan_hash"].as_str().unwrap().into(),
            "--confirm-reset".into(),
            plan["manifest"]["confirmation_phrase"]
                .as_str()
                .unwrap()
                .into(),
        ],
        format!(
            "reset_status=completed\nreset_completed_at={}\nreset_report={}\n",
            result["completed_at"].as_str().unwrap(),
            fixture
                .target
                .join(result["report"]["file"].as_str().unwrap())
                .display()
        ),
    );
}

fn migrations(fixture: &Fixture, maintenance: &Value) {
    let executable = maintenance["artifacts"]["migrate"]["executable"]
        .as_str()
        .unwrap();
    let mut keys = KEYS;
    keys.sort_unstable();
    for (scope, key) in std::iter::once(("control", None))
        .chain(keys.into_iter().map(|key| ("tenant-data", Some(key))))
    {
        for action in ["up", "verify"] {
            let mut command = vec![
                executable.into(),
                "--config-dir".into(),
                fixture.request["config_dir"].as_str().unwrap().into(),
                scope.into(),
                action.into(),
            ];
            let mut stdout = format!("scope={scope} ");
            let mut stage = format!("migrate-{scope}");
            if let Some(key) = key {
                command.extend(["--target".into(), key.into()]);
                stdout.push_str(&format!("target={key} "));
                stage.push_str(&format!("---target-{key}"));
            }
            stdout.push_str(if action == "up" {
                "migration=completed schema=verified\n"
            } else {
                "migration_ledger=current schema=verified\n"
            });
            command_receipt(fixture, &format!("{stage}-{action}"), command, stdout);
        }
    }
}

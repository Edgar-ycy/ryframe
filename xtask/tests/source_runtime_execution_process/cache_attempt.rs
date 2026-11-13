//! 来源缓存尝试所需的隔离失败前缀和完整代次绑定。
use super::invoke;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Output,
};

pub(super) const SCOPE: &str = "source-unit";
const STARTED_AT: &str = "2026-01-01T00:00:00Z";
const PRODUCER_COMPLETED_AT: &str = "2026-01-01T00:00:02Z";

pub(super) struct Fixture {
    pub(super) directory: PathBuf,
    pub(super) intent: PathBuf,
    pub(super) frame: Value,
    pub(super) cleanup: Value,
    evidence: PathBuf,
}

impl Fixture {
    pub(super) fn new() -> Self {
        let backend = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let local = backend.join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        let root = tempfile::Builder::new()
            .prefix("source-cache-attempt-process-")
            .tempdir_in(local)
            .unwrap()
            .keep();
        eprintln!("来源缓存尝试进程证据 {}", root.display());
        let evidence = root.join("evidence");
        fs::create_dir(&evidence).unwrap();
        let run = root.join("copy-run");
        let generation = run.join("g0001");
        let directory = generation.join("verification");
        for name in ["before", "audit", "source-tools"] {
            fs::create_dir_all(directory.join(name)).unwrap();
        }
        let (request, start) = copy_generation(&run);
        let (lineage, subjects) = source_lineage();
        let lineage_binding = write(&generation.join("dataset-lineage.json"), &lineage);
        let before = write(&directory.join("before/image.json"), &json!({"image":{}}));
        let frame = json!({"backend":backend,"directory":directory,"output":directory.join("cache-cleanup.json")});
        let paths = json!({"backend":backend,"directory":directory});
        saved_producer(
            backend,
            &directory,
            &paths,
            &start,
            &lineage_binding,
            &lineage,
            &subjects,
        );
        login_audit(&directory, &paths, &lineage, &subjects);
        write(
            &directory.join("failed.json"),
            &json!({"status":"failed","error":"SourceRuntimeError"}),
        );

        let (observation, comparisons) = frozen_cache(&subjects);
        let pre_manifest = model("runtime-manifest", paths);
        let intent = directory.join("source-verification-recovery-intent.json");
        write(
            &intent,
            &policy(
                "recovery-intent",
                json!({"backend":backend,"directory":directory,
            "start":start,"lineage":lineage_binding,"before":before,"pre_manifest":pre_manifest,
            "successor":{"snapshot":{"clean":true}},"authorization_cache":observation,
            "started_at":"2026-01-01T00:00:03Z"}),
            ),
        );
        let mut arguments = frame.clone();
        arguments["request"] = request;
        arguments["environment"] = json!({"APP_ENV":"test"});
        arguments["observation"] = observation;
        arguments["comparisons"] = comparisons;
        let cleanup =
            json!({"operation":"runtime-cache-cleanup","write":true,"arguments":arguments});
        Self {
            directory,
            intent,
            frame,
            cleanup,
            evidence,
        }
    }

    pub(super) fn save_exchange(&self, name: &str, request: &Value, output: &Output) {
        let directory = self.evidence.join(name);
        fs::create_dir(&directory).unwrap();
        write(&directory.join("request.json"), request);
        write_bytes(&directory.join("stdout.json"), &output.stdout);
        write_bytes(&directory.join("stderr.log"), &output.stderr);
        write(
            &directory.join("exit.json"),
            &json!({"code":output.status.code()}),
        );
    }
}

fn copy_generation(run: &Path) -> (Value, Value) {
    let target = run.join("target");
    fs::create_dir(&target).unwrap();
    fs::create_dir(run.join("results")).unwrap();
    // 故意缺少 Redis 端口，原生客户端在读取配置和连接前即退出。
    let request = json!({"side":"seed","target":{"scope_id":SCOPE}});
    let request_file = write(&target.join("request.json"), &request);
    let prepare = write(
        &target.join("prepare.json"),
        &json!({"request":request_file,"request_sha256":plan_hash(&request)}),
    );
    let initialized = write(
        &target.join("initialized.json"),
        &json!({"status":"fresh_target_initialized","scope_id":SCOPE,"prepare_sha256":prepare["sha256"]}),
    );
    let manifest = write(
        &run.join("manifest.json"),
        &json!({"copy_stage":"source_to_seed","initialized":initialized}),
    );
    let registration = write(
        &run.join("source-registration.json"),
        &json!({"run_manifest":manifest}),
    );
    let registered = write(
        &run.join("registered.json"),
        &json!({"registration":registration}),
    );
    let generation = write(
        &run.join("generation-request.json"),
        &json!({"source_registration":registered}),
    );
    let start = write(
        &run.join("results/0001.json"),
        &json!({"status":"running","generation_id":"g0001",
        "request":generation,"source_registration":registered}),
    );
    (request, start)
}

fn source_lineage() -> (Value, Value) {
    let subjects = (0..11)
        .map(|index| {
            json!({"tenant_id":format!("tenant-{index}"),
        "user_id":(9_007_199_254_740_993_u64 + index).to_string(),"user_authorization_version":1})
        })
        .collect::<Vec<_>>();
    let tenants = subjects
        .iter()
        .map(|subject| json!({"tenant_id":subject["tenant_id"],"username":"owner"}))
        .collect::<Vec<_>>();
    (
        json!({"verification":{"scope_id":SCOPE},"scopes":{"origin_tenant_scope_id":"fixture-origin"},
        "scale":{"tenants":11,"post_samples":33,"business_objects":256},"tenants":tenants}),
        json!(subjects),
    )
}

fn saved_producer(
    backend: &Path,
    directory: &Path,
    paths: &Value,
    start: &Value,
    lineage_binding: &Value,
    lineage: &Value,
    subjects: &Value,
) {
    let tool = Path::new(env!("CARGO_BIN_EXE_xtask"));
    let operation = "a".repeat(32);
    let source_tools = directory.join("source-tools");
    fs::create_dir(source_tools.join("openapi")).unwrap();
    let contract = write(&source_tools.join("openapi/openapi.json"), &json!({}));
    let staging = json!({"contract":"openapi/openapi.json","files":[{"path":"openapi/openapi.json","sha256":contract["sha256"]}],"runtime_modules":[]});
    let staging_file = write(&source_tools.join("manifest.json"), &staging);
    write_bytes(&directory.join("audit/login-before.tsv"), b"old\n");
    let inputs = model("producer-files", paths.clone());
    let configuration_path = directory.join("source-producer-configuration.json");
    let configuration = write(&configuration_path, &json!({}));
    let command = policy(
        "producer-command",
        json!({"staging":source_tools,"backend":backend,"execution":backend,
        "directory":directory,"operation":operation,"tool":tool,"start":start,"lineage":lineage_binding,"configuration":configuration_path}),
    );
    let controller = json!({"pid":100,"started":"fixture-controller","executable":tool});
    let process = json!({"pid":101,"started":"fixture-producer","executable":tool});
    let intent = write(
        &directory.join("source-producer-intent.json"),
        &json!({"format_version":2,"kind":"restore-source-producer-intent",
        "operation_id":operation,"controller":controller,"source_generation":start,"dataset_lineage":lineage_binding,
        "executable":tool,"source":binding(&backend.join("xtask/src/recovery/dataset_prepare/existing_native.rs")),
        "configuration":configuration,"argv":command["argv"],"cwd":backend,
        "environment_sha256":plan_hash(&json!({"APP_ENV":"test"})),"started_at":STARTED_AT,
        "coordinator_source":{"snapshot":{"clean":true}},"execution_sha":"a".repeat(40),"staging":staging_file,"inputs":inputs}),
    );
    let producer = write(
        &directory.join("source-producer.json"),
        &json!({"format_version":1,"kind":"restore-source-producer",
        "operation_id":operation,"intent":intent,"controller":controller,"process":process,"executable":tool,"argv":command["argv"]}),
    );
    let ready = json!({"format_version":1,"kind":"restore-source-producer-ready","operation_id":operation,
        "source_generation_sha256":start["sha256"],"lineage_file_sha256":lineage_binding["sha256"],
        "staging_manifest_sha256":staging_file["sha256"],"runtime_files_sha256":plan_hash(&staging["runtime_modules"]),
        "contract_file_sha256":contract["sha256"]});
    let ready_path = directory.join("source-producer-ready.json");
    write_bytes(&ready_path, format!("{ready}\n").as_bytes());
    let stdout = write(
        &directory.join("source-producer.stdout.json"),
        &json!({"format_version":1,"kind":"restore-source-existing-verification",
        "status":"source_existing_data_verified","scope_id":SCOPE,"origin_tenant_scope_id":lineage["scopes"]["origin_tenant_scope_id"],
        "lineage_sha256":plan_hash(lineage),"actions":{"business":"read_only","objects":"read_only","session":"login_logout"},
        "restore_success":false,"subjects":subjects,"tenants":11,"posts":33,"files":256,
        "source_generation_sha256":start["sha256"],"lineage_file_sha256":lineage_binding["sha256"]}),
    );
    let stderr = directory.join("source-producer.stderr.log");
    write_bytes(&stderr, b"");
    write(
        &directory.join("source-producer-completion.json"),
        &json!({"format_version":1,"kind":"restore-source-producer-completion",
        "operation_id":operation,"intent":intent,"producer":producer,"ready":binding(&ready_path),
        "stdout":stdout,"stderr":binding(&stderr),"process":process,"exit_code":0,"completed_at":PRODUCER_COMPLETED_AT}),
    );
}

fn login_audit(directory: &Path, paths: &Value, lineage: &Value, subjects: &Value) {
    let rows = subjects
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(index, subject)| {
            [
                (index + 11).to_string(),
                hex::encode_upper(subject["tenant_id"].as_str().unwrap()),
                hex::encode_upper("owner"),
                hex::encode_upper(format!("198.18.20.{}", index + 1)),
                "NULL".into(),
                "NULL".into(),
                "NULL".into(),
                "31".into(),
                "NULL".into(),
                "2026-01-01T00:00:01.000000".into(),
            ]
            .join("\t")
        })
        .collect::<Vec<_>>()
        .join("\n");
    write_bytes(&directory.join("audit/login-after-old.tsv"), b"old\n");
    write_bytes(
        &directory.join("audit/login-after-new.tsv"),
        format!("{rows}\n").as_bytes(),
    );
    let value = policy(
        "login-create",
        json!({"backend":paths["backend"],"directory":directory,
        "old_before":hex::encode(b"old\n"),"old_after":hex::encode(b"old\n"),"upper_id":10,"new_raw":rows,
        "lineage":lineage,"subjects":subjects,"started_at":STARTED_AT,"completed_at":PRODUCER_COMPLETED_AT}),
    );
    write(&directory.join("audit/login-audit.json"), &value);
}

fn frozen_cache(subjects: &Value) -> (Value, Value) {
    let namespace = format!("ryframe:{{{SCOPE}}}:");
    let plan = model(
        "cache-plan",
        json!({"namespace":namespace,"subjects":subjects}),
    );
    let raw = subjects.as_array().unwrap().iter().map(|subject| {
        let payload = json!({"versions":{"tenant_authorization_epoch":2,"user_authorization_version":1,"data_scope_version":1},"tenant_session_version":1,
            "principal":{"actor":{"user_id":subject["user_id"],"tenant_id":subject["tenant_id"],"username":"owner","dept_id":null,
                "dept_path":null,"data_scope":"all","custom_dept_ids":[],"include_self":true,"data_scope_version":1,"is_super_admin":false},
                "password_change_required":false,"user_authorization_version":1,"data_scope_version":1,"tenant_authorization_epoch":2,
                "preferred_locale":null,"roles":[],"role_ids":[],"permissions":[],"tenant_request_limit_per_minute":600}}).to_string();
        json!({"types":{"tenant_epoch":"string","user_version":"string","snapshot":"hash"},
            "tenant_epoch":"2","user_version":"1","fields":["2:1"],"payload":payload})
    }).collect::<Vec<_>>();
    let observed = policy("cache-observe", json!({"plan":plan,"raw":raw}));
    (
        json!({"namespace":namespace,"subjects":subjects,"plan":plan,"before":observed["before"]}),
        observed["comparisons"].clone(),
    )
}

fn policy(action: &str, arguments: Value) -> Value {
    model(
        "producer-ready",
        json!({"action":action,"arguments":arguments}),
    )
}

fn model(operation: &str, arguments: Value) -> Value {
    let (status, response) =
        invoke(&json!({"operation":operation,"write":false,"arguments":arguments}));
    assert!(status.success(), "{operation}: {response}");
    response["result"]["value"].clone()
}

fn plan_hash(value: &Value) -> String {
    hex::encode(Sha256::digest(serde_json::to_vec(value).unwrap()))
}

pub(super) fn binding(path: &Path) -> Value {
    let bytes = fs::read(path).unwrap();
    let path = path.to_str().unwrap();
    #[cfg(windows)]
    let path = path.replace('/', "\\");
    json!({"path":path,"bytes":bytes.len(),"sha256":hex::encode(Sha256::digest(bytes))})
}

pub(super) fn read(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn write(path: &Path, value: &Value) -> Value {
    write_bytes(path, &serde_json::to_vec(value).unwrap());
    binding(path)
}

fn write_bytes(path: &Path, bytes: &[u8]) {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .unwrap()
        .write_all(bytes)
        .unwrap();
}

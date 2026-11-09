use std::{path::PathBuf, sync::Mutex};

use sha2::{Digest, Sha256};

use super::*;
use crate::recovery::clone::native::{
    export::transport::{CommandOutput, CommandSpec},
    s3::{Operation, Request},
};

const SOURCE: &str = "fixture-original";
const TARGET: &str = "fixture-target";
const ENDPOINT: &str = "http://127.0.0.1:29602";
const REGION: &str = "us-east-1";
const BUSINESS: &str = "fixture-target/business.bin";
const EXTRA: &str = "fixture-target/extra.bin";
const EXPORT: &str = "fixture-target/export.bin";

struct Fixture {
    root: tempfile::TempDir,
    backend: PathBuf,
    execution: PathBuf,
    output: PathBuf,
    request: Value,
    plan: Value,
    selected: Value,
    configuration: Value,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir_in(crate::workspace::root_dir()).unwrap();
        let backend = root.path().join("coordinator");
        let execution = root.path().join("target-execution");
        let originals = backend.join(".local-tests/originals");
        let output = backend.join(".local-tests/attempt");
        let config = execution.join(".local-tests/target-config");
        for directory in [&originals, &output, &config] {
            fs::create_dir_all(directory).unwrap();
        }
        fs::write(config.join("app.test.toml"), format!(
            "[object_storage]\nendpoint={ENDPOINT:?}\nregion={REGION:?}\naccess_key=\"private-fixture\"\nsecret_key=\"private-fixture\"\n"
        )).unwrap();
        let configuration =
            app_configuration::configuration_value(&execution, &config, "test").unwrap();
        let plan = json!({"copy_stage":"seed_to_arm","source_scope":"fixture-seed",
            "target_scope":TARGET,"plan_sha256":"a".repeat(64),"objects":[
                item("uploads",BUSINESS),item("uploads",EXTRA),item("exports",EXPORT)]});
        let reference = json!({"source":{"scope_id":SOURCE}});
        let dataset = json!({"format_version":1,"plan_sha256":digest::plan_hash(&reference).unwrap(),
            "source_scope_id":SOURCE,"tenants":[{"posts":[{"id":"1"}],
            "files":[{"file_path":"business.bin","bytes":4,"sha256":object_sha()}]}]});
        let result = json!({"status":"data_steps_verified","plan_sha256":plan["plan_sha256"],
            "generation_sha256":"b".repeat(64),"source_export_sha256":"c".repeat(64),
            "fresh_target_sha256":"d".repeat(64)});
        let mut stage = result.clone();
        stage["continuation_receipt"] = json!("ignored-by-original-comparison");
        let ledger = json!({"format_version":1,"plan_sha256":result["plan_sha256"],
            "generation_sha256":result["generation_sha256"],"sequence":5,"sha256":"e".repeat(64)});
        let mut request = json!({});
        for (field, value) in [
            ("reference_plan", reference),
            ("dataset", dataset),
            ("copy_result", result),
            ("copy_stage_receipt", stage),
            ("ledger_head", ledger),
        ] {
            request[field] = save(&originals.join(format!("{field}.json")), &value);
        }
        Self {
            root,
            backend,
            execution,
            output,
            request,
            plan,
            configuration,
            selected: json!({"scope_id":TARGET,"api_url":"http://127.0.0.1:29601",
                "frontend_url":"http://127.0.0.1:29603/"}),
        }
    }

    fn prepare(&self) -> Result<Business<'_>> {
        prepare(
            &self.backend,
            &self.output,
            &self.request,
            &self.plan,
            &self.selected,
        )
    }

    fn access(&self) -> ObjectRead<'_> {
        ObjectRead {
            configuration_root: &self.execution,
            configuration: &self.configuration,
            endpoint: ENDPOINT,
            region: REGION,
        }
    }

    fn originals(&self, name: &str) -> PathBuf {
        self.backend
            .join(".local-tests/originals")
            .join(format!("{name}.json"))
    }
}

fn save(path: &Path, value: &Value) -> Value {
    document::write_new(path, value).unwrap();
    common::binding(path).unwrap()
}

fn object_sha() -> String {
    hex::encode(Sha256::digest(b"data"))
}

fn item(bucket: &str, key: &str) -> Value {
    json!({"bucket":bucket,"target_key":key,"artifact":{"bytes":4,"sha256":object_sha()}})
}

fn observed(business: &Business<'_>) -> Value {
    json!({"format_version":1,"status":"copy_existing_data_verified","side":"copy_target",
        "scope_id":business.target["target"]["scope_id"],"source_scope_id":business.target["source_scope_id"],
        "plan_sha256":business.target["source_plan_sha256"],"target_binding_sha256":business.target_sha256,
        "copy":business.target["copy"],"input_files":business.input_files,
        "actions":{"business":"read_only","objects":"read_only","session":"login_logout"},
        "clone_verified":false,"restore_success":false,"tenants":1,"posts":1,"files":1})
}

#[derive(Default)]
struct Mock {
    extra_in_list: bool,
    missing_after_list: bool,
    keys: Mutex<Vec<String>>,
    downloads: Mutex<Vec<String>>,
    listed: Mutex<Vec<String>>,
}

impl Transport for Mock {
    fn run(&self, _: CommandSpec) -> Result<CommandOutput> {
        panic!("复制后对象核验不得启动外部命令");
    }

    fn s3(&self, request: Request) -> Result<Value> {
        let owner = format!("{TARGET}/.ryframe-owner");
        let headers = || {
            json!({"ContentType":"text/plain","ContentLength":4,"Metadata":{},
            "ETag":"\"fixture-etag\"","LastModified":"2026-11-09T00:00:00+00:00"})
        };
        match request.operation {
            Operation::List {
                prefix,
                continuation,
            } => {
                assert_eq!(prefix, format!("{TARGET}/"));
                assert!(continuation.is_none());
                self.listed.lock().unwrap().push(request.bucket.clone());
                let mut keys = vec![json!({"Key":owner})];
                match request.bucket.as_str() {
                    "uploads" => keys.extend([json!({"Key":BUSINESS}), json!({"Key":EXTRA})]),
                    "exports" => keys.push(json!({"Key":EXPORT})),
                    "avatar" if self.extra_in_list => {
                        keys.push(json!({"Key":format!("{TARGET}/unexpected.bin")}))
                    }
                    _ => (),
                }
                Ok(json!({"IsTruncated":false,"Contents":keys}))
            }
            Operation::Head { key, .. } => {
                assert_ne!(key, BUSINESS, "业务对象由 existing producer 核验");
                self.keys.lock().unwrap().push(key.clone());
                if self.missing_after_list && key == EXTRA {
                    return Ok(json!({"status":"not_found","http_status":404,
                        "remote_write_may_have_occurred":false}));
                }
                Ok(headers())
            }
            Operation::Get {
                key, destination, ..
            } => {
                if key == owner {
                    let body = format!(
                        "ryframe-owner:v1:{TARGET}:object-storage:{}",
                        request.bucket
                    );
                    fs::write(destination, &body)?;
                    return Ok(json!({"ContentLength":body.len()}));
                }
                assert_ne!(key, BUSINESS, "业务对象由 existing producer 核验");
                self.downloads.lock().unwrap().push(key);
                fs::write(destination, b"data")?;
                Ok(headers())
            }
            _ => panic!("复制后对象核验只允许 S3 读取"),
        }
    }
}

#[test]
fn verified_producer_and_full_scope_download_only_the_object_complement() {
    let fixture = Fixture::new();
    let business = fixture.prepare().unwrap();
    assert_eq!(fixture.root.path(), fixture.backend.parent().unwrap());
    let target_path = fixture.output.join("business-target.json");
    assert_eq!(document::read(&target_path).unwrap(), business.target);
    assert_eq!(
        business.input_files["target_binding_sha256"],
        digest::file(&target_path).unwrap().sha256
    );
    assert_eq!(business.target["source_scope_id"], SOURCE);
    assert_eq!(
        business.target["copy"]["ledger_head_sha256"],
        fixture.request["ledger_head"]["sha256"]
    );
    assert_ne!(
        business.target["source_plan_sha256"],
        business.input_files["plan_sha256"]
    );
    assert_ne!(
        business.target_sha256,
        business.input_files["target_binding_sha256"]
    );
    business.check().unwrap();
    let producer = observed(&business);
    let mock = Mock::default();
    let result = business
        .finish(&producer, fixture.access(), &mock, &|| Ok(()))
        .unwrap();
    assert_eq!(
        result.objects,
        json!({"business_objects":1,"additional_objects":2,
        "all_scoped_keys_verified":true,"additional_objects_downloaded":true})
    );
    assert_eq!(
        common::read_bound(&fixture.backend, &result.evidence, "测试结果").unwrap(),
        producer
    );
    assert_eq!(
        *mock.listed.lock().unwrap(),
        ["avatar", "config-packages", "exports", "imports", "uploads"]
    );
    assert_eq!(*mock.downloads.lock().unwrap(), [EXPORT, EXTRA]);
}

#[test]
fn producer_must_match_raw_digests_and_the_complete_business_sample() {
    let fixture = Fixture::new();
    let business = fixture.prepare().unwrap();
    let producer = observed(&business);
    let mut wrong = producer.clone();
    wrong["input_files"]["plan_sha256"] = producer["plan_sha256"].clone();
    let mock = Mock::default();
    assert!(
        business
            .finish(&wrong, fixture.access(), &mock, &|| Ok(()))
            .is_err()
    );
    wrong = producer;
    wrong["files"] = json!(0);
    assert!(
        business
            .finish(&wrong, fixture.access(), &mock, &|| Ok(()))
            .is_err()
    );
    assert!(mock.listed.lock().unwrap().is_empty());
    assert!(!fixture.output.join("business-result.json").exists());
}

#[test]
fn registered_dataset_uploads_must_bind_the_original_copy_plan_exactly() {
    let mut fixture = Fixture::new();
    let path = fixture.originals("dataset");
    let mut dataset = document::read(&path).unwrap();
    dataset["tenants"][0]["files"][0]["sha256"] = json!("f".repeat(64));
    fs::write(&path, serde_json::to_vec(&dataset).unwrap()).unwrap();
    fixture.request["dataset"] = common::binding(&path).unwrap();
    assert!(fixture.prepare().is_err());
    dataset["tenants"][0]["files"][0]["sha256"] = json!(object_sha());
    let files = dataset["tenants"][0]["files"].as_array_mut().unwrap();
    files.push(files[0].clone());
    fs::write(&path, serde_json::to_vec(&dataset).unwrap()).unwrap();
    fixture.request["dataset"] = common::binding(&path).unwrap();
    assert!(fixture.prepare().is_err());
    assert!(!fixture.output.join("business-target.json").exists());
}

#[test]
fn unexpected_keys_or_post_list_absence_fail_and_preserve_verified_producer_result() {
    for extra_in_list in [false, true] {
        let fixture = Fixture::new();
        let business = fixture.prepare().unwrap();
        let producer = observed(&business);
        let mock = Mock {
            extra_in_list,
            missing_after_list: !extra_in_list,
            ..Mock::default()
        };
        assert!(
            business
                .finish(&producer, fixture.access(), &mock, &|| Ok(()))
                .is_err()
        );
        assert_eq!(
            document::read(&fixture.output.join("business-result.json")).unwrap(),
            producer
        );
        if extra_in_list {
            assert!(mock.downloads.lock().unwrap().is_empty());
            assert!(fixture.output.join("object-scope/failure.json").exists());
        } else {
            assert_eq!(*mock.downloads.lock().unwrap(), [EXPORT]);
            assert!(
                fixture
                    .output
                    .join("remaining-object-0001/head-after.json")
                    .exists()
            );
        }
    }
}

use std::{
    fs,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use sha2::{Digest, Sha256};

use super::*;
use crate::recovery::{
    clone::native::{
        export::transport::{CommandOutput, CommandSpec},
        lifecycle::manifest,
        s3::{Operation, Request, app_configuration},
    },
    source::evidence::inspect_object,
};

const SOURCE: &str = "fixture-source";
const TARGET: &str = "fixture-target";
const SOURCE_ENDPOINT: &str = "http://127.0.0.1:29601";
const TARGET_ENDPOINT: &str = "http://127.0.0.1:29602";
const REGION: &str = "us-east-1";
const BUCKET: &str = "uploads";

struct Fixture {
    root: tempfile::TempDir,
    target_root: PathBuf,
    export_root: PathBuf,
    work: PathBuf,
    source_configuration: Value,
    target_configuration: Value,
    export: Value,
    proof_files: Value,
    captures: BTreeMap<String, BTreeMap<String, Value>>,
    items: Vec<Value>,
    plan: String,
    generation: String,
}

impl Fixture {
    fn new(count: usize) -> Self {
        let root = tempfile::tempdir_in(crate::workspace::root_dir()).unwrap();
        let export_root = root.path().join(".local-tests/export");
        let work = root.path().join(".local-tests/work");
        fs::create_dir_all(&work).unwrap();
        let source_configuration = config(root.path(), "source", SOURCE_ENDPOINT);
        let target_root = root.path().join("target-execution");
        let target_configuration = config(&target_root, "target", TARGET_ENDPOINT);
        assert!(
            app_configuration::public_configuration(root.path(), &target_configuration).is_err()
        );
        let mut captures = BUCKETS
            .iter()
            .map(|&bucket| (bucket.to_owned(), BTreeMap::new()))
            .collect::<BTreeMap<_, _>>();
        let mut proof_files = Map::new();
        let mut originals = Vec::new();
        let mut items = Vec::new();
        for index in 0..count {
            let (item, original, capture) = object(&export_root, index, &mut proof_files);
            captures
                .get_mut(BUCKET)
                .unwrap()
                .insert(item["source_key"].as_str().unwrap().to_owned(), capture);
            originals.push(original);
            items.push(item);
        }
        let buckets = BUCKETS
            .iter()
            .map(|&bucket| {
                json!({"bucket":bucket,
            "entries":if bucket == BUCKET {originals.as_slice()} else {&[]}})
            })
            .collect::<Vec<_>>();
        let export_path = export_root.join("export.json");
        document::write_new(
            &export_path,
            &json!({"scope_id":SOURCE,
            "source":{"s3":{"endpoint":SOURCE_ENDPOINT,"region":REGION}},"objects":buckets}),
        )
        .unwrap();
        let export = common::binding(&export_path).unwrap();
        Self {
            root,
            target_root,
            export_root,
            work,
            source_configuration,
            target_configuration,
            export,
            proof_files: Value::Object(proof_files),
            captures,
            items,
            plan: "a".repeat(64),
            generation: "b".repeat(64),
        }
    }

    fn inputs(&self) -> ObjectInputs<'_> {
        ObjectInputs {
            backend: self.root.path(),
            target_configuration_root: &self.target_root,
            work: &self.work,
            export_root: &self.export_root,
            source: Side {
                scope: SOURCE,
                endpoint: SOURCE_ENDPOINT,
                region: REGION,
                configuration: &self.source_configuration,
            },
            target: Side {
                scope: TARGET,
                endpoint: TARGET_ENDPOINT,
                region: REGION,
                configuration: &self.target_configuration,
            },
            export: &self.export,
            captures: &self.captures,
            proof_files: &self.proof_files,
            plan_sha256: &self.plan,
            generation_sha256: &self.generation,
        }
    }

    fn step(&self, index: usize) -> Step<'_> {
        let item = &self.items[index];
        let ids = copy_step_ids(
            &json!({"databases":[],"objects":[{"bucket":BUCKET,"target_key":item["target_key"]}]}),
        )
        .unwrap();
        Step {
            id: ids["objects"][0].as_str().unwrap().to_owned(),
            item,
            kind: Kind::Object,
            artifact: item["artifact"]["sha256"].as_str().unwrap(),
        }
    }
}

fn config(root: &Path, side: &str, endpoint: &str) -> Value {
    let path = root.join(".local-tests").join(side);
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join("app.test.toml"), format!(
        "[object_storage]\nendpoint={endpoint:?}\nregion={REGION:?}\naccess_key=\"private-example\"\nsecret_key=\"private-example\"\n"
    )).unwrap();
    app_configuration::configuration_value(root, &path, "test").unwrap()
}

fn object(root: &Path, index: usize, proofs: &mut Map<String, Value>) -> (Value, Value, Value) {
    let source = format!("{SOURCE}/object-{index}.bin");
    let target = format!("{TARGET}/object-{index}.bin");
    let directory = root.join(format!("objects/item-{index}"));
    fs::create_dir_all(&directory).unwrap();
    let raw = headers();
    let inspected = inspect_object(&raw).unwrap();
    let capture = json!({"identity":inspected["identity"],"metadata":inspected["metadata"],
        "response_sha256":{"head_before":digest::plan_hash(&raw).unwrap()}});
    document::write_new(&directory.join("capture.json"), &capture).unwrap();
    document::write_new(&directory.join("head-before.json"), &raw).unwrap();
    fs::write(directory.join("head-before.diagnostic.json"), b"saved\n").unwrap();
    fs::write(directory.join("object.bin"), b"data").unwrap();
    for name in [
        "capture.json",
        "head-before.json",
        "head-before.diagnostic.json",
    ] {
        let path = directory.join(name);
        let value = digest::file(&path).unwrap();
        proofs.insert(
            manifest::path_text(&path, "测试路径").unwrap(),
            json!({"bytes":value.bytes,"sha256":value.sha256}),
        );
    }
    let receipt = digest::file(&directory.join("capture.json")).unwrap();
    let artifact = json!({"file":format!("objects/item-{index}/object.bin"),"bytes":4,
        "sha256":hex::encode(Sha256::digest(b"data"))});
    let original = json!({"key":source,"artifact":artifact,"metadata":inspected["metadata"],
        "capture":{"file":format!("objects/item-{index}/capture.json"),"bytes":receipt.bytes,"sha256":receipt.sha256}});
    let item = json!({"bucket":BUCKET,"source_key":source,"target_key":target,
        "artifact":artifact,"metadata":inspected["metadata"]});
    (item, original, capture)
}

fn headers() -> Value {
    json!({"ContentType":"text/plain","ContentLength":4,"Metadata":{},"ETag":"\"fixture-etag\"",
        "VersionId":"fixture-version","LastModified":"2026-11-09T00:00:00+00:00"})
}

#[derive(Default)]
struct Mock {
    present: bool,
    source_error: bool,
    calls: Mutex<Vec<String>>,
    business_gets: AtomicUsize,
}

impl Transport for Mock {
    fn run(&self, _: CommandSpec) -> Result<CommandOutput> {
        panic!("对象观察不得启动外部命令");
    }

    fn s3(&self, request: Request) -> Result<Value> {
        match request.operation {
            Operation::Head {
                key,
                if_match,
                version,
            } => {
                self.calls.lock().unwrap().push(key.clone());
                if key.starts_with(SOURCE) && self.source_error {
                    return Err("测试来源 HEAD 失败".into());
                }
                if key.starts_with(SOURCE) || if_match.is_some() {
                    assert_eq!(if_match.as_deref(), Some("\"fixture-etag\""));
                    assert_eq!(version.as_deref(), Some("fixture-version"));
                }
                if key.starts_with(TARGET) && !self.present {
                    return Ok(
                        json!({"status":"not_found","http_status":404,"remote_write_may_have_occurred":false}),
                    );
                }
                Ok(headers())
            }
            Operation::Get {
                key, destination, ..
            } => {
                self.calls.lock().unwrap().push(key.clone());
                if let Some(scope) = key.strip_suffix("/.ryframe-owner") {
                    let owner =
                        format!("ryframe-owner:v1:{scope}:object-storage:{}", request.bucket);
                    fs::write(destination, &owner)?;
                    return Ok(json!({"ContentLength":owner.len()}));
                }
                assert!(key.starts_with(TARGET) && self.present);
                self.business_gets.fetch_add(1, Ordering::SeqCst);
                fs::write(destination, b"data")?;
                Ok(headers())
            }
            Operation::List {
                prefix,
                continuation,
            } => {
                assert!(continuation.is_none());
                let mut keys = vec![json!({"Key":format!("{prefix}.ryframe-owner")})];
                if request.bucket == BUCKET {
                    keys.push(json!({"Key":format!("{prefix}object-0.bin")}));
                }
                Ok(json!({"IsTruncated":false,"Contents":keys}))
            }
            _ => panic!("只读观察不得请求对象写入"),
        }
    }
}

#[test]
fn basis_checks_original_small_proofs_and_selected_artifact() {
    let fixture = Fixture::new(1);
    let mock = Mock::default();
    let port = ObjectPort::new(fixture.inputs(), &mock).unwrap();
    let step = fixture.step(0);
    let basis = port.basis(&step).unwrap();
    assert_eq!(basis.before["exists"], false);
    assert_eq!(basis.after["sha256"], step.artifact);
    let directory = fixture.export_root.join("objects/item-0");
    let diagnostic = directory.join("head-before.diagnostic.json");
    fs::write(&diagnostic, b"other\n").unwrap();
    assert!(port.basis(&step).is_err());
    fs::write(diagnostic, b"saved\n").unwrap();
    fs::write(directory.join("object.bin"), b"evil").unwrap();
    assert!(port.basis(&step).is_err());
    assert!(mock.calls.lock().unwrap().is_empty());
}

#[test]
fn batch_completes_source_before_target_and_rechecks_unchanged_local_proofs() {
    let fixture = Fixture::new(2);
    let mock = Mock::default();
    let mut port = ObjectPort::new(fixture.inputs(), &mock).unwrap();
    let steps = [fixture.step(0), fixture.step(1)];
    assert!(port.observe(&[&steps[0], &steps[0]], &|| Ok(())).is_err());
    let observation = port.observe(&[&steps[0], &steps[1]], &|| Ok(())).unwrap();
    {
        let calls = mock.calls.lock().unwrap();
        assert_eq!(calls.len(), 14);
        assert!(calls[..6].iter().all(|key| key.starts_with(SOURCE)));
        assert!(calls[6..].iter().all(|key| key.starts_with(TARGET)));
    }
    assert_eq!(observation.images[&steps[0].id]["exists"], false);
    assert_eq!(observation.images[&steps[1].id]["exists"], false);
    port.check_observation(&observation.images, &observation.evidence)
        .unwrap();
    let sample = &observation.evidence.0[0];
    let path = Path::new(sample.target_descriptor["path"].as_str().unwrap())
        .with_file_name("head-after.json");
    fs::write(path, b"{}\n").unwrap();
    assert!(
        port.check_observation(&observation.images, &observation.evidence)
            .is_err()
    );
    assert_eq!(mock.calls.lock().unwrap().len(), 14);
}

#[test]
fn final_target_heads_and_source_snapshot_do_not_download_another_business_body() {
    let fixture = Fixture::new(1);
    let mock = Mock {
        present: true,
        ..Mock::default()
    };
    let mut port = ObjectPort::new(fixture.inputs(), &mock).unwrap();
    let step = fixture.step(0);
    let observation = port.observe(&[&step], &|| Ok(())).unwrap();
    port.check_observation(&observation.images, &observation.evidence)
        .unwrap();
    assert_eq!(mock.business_gets.load(Ordering::SeqCst), 1);
    assert_eq!(
        port.verify_source(&|| Ok(())).unwrap()["whole_source"],
        true
    );
    let target = port.verify_target(&fixture.items, &|| Ok(())).unwrap();
    assert_eq!(target["status"], "target_object_heads_verified");
    assert_eq!(mock.business_gets.load(Ordering::SeqCst), 1);
}

#[test]
fn failed_source_group_drains_without_starting_any_target_observation() {
    let fixture = Fixture::new(2);
    let mock = Mock {
        source_error: true,
        ..Mock::default()
    };
    let mut port = ObjectPort::new(fixture.inputs(), &mock).unwrap();
    let steps = [fixture.step(0), fixture.step(1)];
    assert!(port.observe(&[&steps[0], &steps[1]], &|| Ok(())).is_err());
    let calls = mock.calls.lock().unwrap();
    assert_eq!(calls.len(), 4);
    assert!(calls.iter().all(|key| key.starts_with(SOURCE)));
    assert!(port.latest.is_empty());
}

#[test]
fn preledger_target_owner_gate_observes_every_bucket_and_no_business_object() {
    let fixture = Fixture::new(1);
    let mock = Mock::default();
    let port = ObjectPort::new(fixture.inputs(), &mock).unwrap();
    let receipt = port.verify_target_owners(&|| Ok(())).unwrap();
    assert_eq!(receipt["status"], "target_owners_verified");
    assert_eq!(receipt["owners"].as_array().unwrap().len(), 5);
    assert_eq!(
        mock.calls.lock().unwrap().as_slice(),
        vec![format!("{TARGET}/.ryframe-owner"); 5]
    );
    assert_eq!(mock.business_gets.load(Ordering::SeqCst), 0);
}

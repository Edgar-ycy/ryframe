use std::sync::{
    Condvar, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use super::*;
use crate::recovery::clone::native::{
    copy_transfer::observe_object::{ObserveTarget, observe},
    export::transport::{CommandOutput, CommandSpec},
    s3::Request,
};

const SCOPE: &str = "fixture-target";
const ENDPOINT: &str = "http://127.0.0.1:29600";
const REGION: &str = "us-east-1";
const BUCKET: &str = "uploads";
const ETAG: &str = "\"fixture-etag\"";
const PLAN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const GENERATION: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

struct Fixture {
    temp: tempfile::TempDir,
    work: PathBuf,
    directory: PathBuf,
    configuration: Value,
    items: Vec<Value>,
    observations: BTreeMap<(String, String), Observed>,
}

impl Fixture {
    fn new(objects: usize) -> Self {
        let temp = tempfile::tempdir_in(crate::workspace::root_dir()).unwrap();
        let root = temp.path();
        let config = root.join(".local-tests/config");
        fs::create_dir_all(&config).unwrap();
        fs::write(config.join("app.test.toml"), format!(
            "[object_storage]\nendpoint={ENDPOINT:?}\nregion={REGION:?}\naccess_key=\"fixture-private-access\"\nsecret_key=\"fixture-private-secret\"\n"
        )).unwrap();
        let configuration = app_configuration::configuration_value(root, &config, "test").unwrap();
        let work = root.join(".local-tests/work");
        fs::create_dir(&work).unwrap();
        let directory = work.join("final");
        let mut items = Vec::new();
        let mut observations = BTreeMap::new();
        let proof_body = work.join("body-sample.bin");
        fs::write(&proof_body, b"data").unwrap();
        let expected = json!({"bytes":4,"sha256":digest::file(&proof_body).unwrap().sha256});
        for index in 0..objects {
            let target = key(index);
            let evidence = work.join(format!("capture-{index}"));
            let observed = observe(
                ObserveTarget {
                    backend: root,
                    configuration_root: root,
                    work: &work,
                    directory: &evidence,
                    plan_sha256: PLAN,
                    scope: SCOPE,
                    endpoint: ENDPOINT,
                    region: REGION,
                    configuration: &configuration,
                    bucket: BUCKET,
                    key: &target,
                    expected: &expected,
                    max_bytes: 4,
                },
                &Fake::new(objects, Fault::None, false),
                &|| Ok(()),
            )
            .unwrap();
            observations.insert((BUCKET.into(), target.clone()), observed);
            items.push(json!({
                "bucket":BUCKET,"source_key":format!("fixture-source/item-{index}"),
                "target_key":target,"metadata":inspect_object(&raw()).unwrap()["metadata"],
                "artifact":{"file":format!("objects/item-{index}.bin"),"bytes":4,"sha256":expected["sha256"]},
            }));
        }
        Self {
            temp,
            work,
            directory,
            configuration,
            items,
            observations,
        }
    }

    fn request(&self) -> VerifyTarget<'_> {
        VerifyTarget {
            backend: self.temp.path(),
            configuration_root: self.temp.path(),
            work: &self.work,
            directory: &self.directory,
            plan_sha256: PLAN,
            generation_sha256: GENERATION,
            scope: SCOPE,
            endpoint: ENDPOINT,
            region: REGION,
            configuration: &self.configuration,
            items: &self.items,
            observations: &self.observations,
        }
    }

    fn result(&self, name: &str) -> Value {
        document::read(&self.directory.join(name)).unwrap()
    }
}

fn key(index: usize) -> String {
    format!("{SCOPE}/item-{index}")
}

fn raw() -> Value {
    json!({"ContentType":"text/plain","ContentLength":4,"Metadata":{},"ETag":ETAG,
        "LastModified":"2026-11-09T00:00:00+00:00"})
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fault {
    None,
    RawHead,
    HeadError,
    ExtraKey,
    FinalOwner,
}

struct Fake {
    objects: usize,
    fault: Fault,
    finalizing: bool,
    heads: Mutex<Vec<usize>>,
    lists: Mutex<Vec<String>>,
    owners: Mutex<Vec<String>>,
    gate: Condvar,
    active: AtomicUsize,
    peak: AtomicUsize,
}

impl Fake {
    fn new(objects: usize, fault: Fault, finalizing: bool) -> Self {
        Self {
            objects,
            fault,
            finalizing,
            heads: Mutex::new(Vec::new()),
            lists: Mutex::new(Vec::new()),
            owners: Mutex::new(Vec::new()),
            gate: Condvar::new(),
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
        }
    }
}

impl Transport for Fake {
    fn run(&self, _: CommandSpec) -> Result<CommandOutput> {
        panic!("收尾不得启动外部命令");
    }

    fn s3(&self, request: Request) -> Result<Value> {
        match request.operation {
            Operation::Head {
                key: target,
                if_match,
                version,
            } => {
                if !self.finalizing {
                    return Ok(raw());
                }
                assert_eq!(request.bucket, BUCKET);
                assert_eq!(if_match.as_deref(), Some(ETAG));
                assert!(version.is_none());
                let index = target.rsplit_once('-').unwrap().1.parse::<usize>().unwrap();
                let mut heads = self.heads.lock().unwrap();
                heads.push(index);
                let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.peak.fetch_max(active, Ordering::SeqCst);
                let end = (index / 4 + 1) * 4;
                if end <= self.objects {
                    self.gate.notify_all();
                    let (_guard, timeout) = self
                        .gate
                        .wait_timeout_while(heads, std::time::Duration::from_secs(5), |started| {
                            started.len() < end
                        })
                        .unwrap();
                    assert!(!timeout.timed_out(), "目标同批四路 HEAD 未到达");
                }
                self.active.fetch_sub(1, Ordering::SeqCst);
                if index == 0 && self.fault == Fault::HeadError {
                    return Err("private-fixture-secret upstream error".into());
                }
                let mut response = raw();
                if index == 0 && self.fault == Fault::RawHead {
                    // 解析身份和元数据相同，原响应不同。
                    response["AcceptRanges"] = json!("bytes");
                }
                Ok(response)
            }
            Operation::List {
                prefix,
                continuation,
            } => {
                assert!(self.finalizing);
                assert_eq!(prefix, format!("{SCOPE}/"));
                assert!(continuation.is_none());
                self.lists.lock().unwrap().push(request.bucket.clone());
                let mut entries = vec![json!({"Key":format!("{SCOPE}/.ryframe-owner")})];
                if request.bucket == BUCKET {
                    entries.extend((0..self.objects).map(|index| json!({"Key":key(index)})));
                    if self.fault == Fault::ExtraKey {
                        entries.push(json!({"Key":format!("{SCOPE}/extra")}));
                    }
                }
                Ok(json!({"IsTruncated":false,"Contents":entries}))
            }
            Operation::Get {
                key: target,
                destination,
                max_bytes,
                ..
            } => {
                if target == format!("{SCOPE}/.ryframe-owner") {
                    assert_eq!(max_bytes, 4096);
                    let name = destination.file_name().unwrap().to_string_lossy();
                    if self.finalizing {
                        self.owners.lock().unwrap().push(name.into_owned());
                    }
                    let mut body =
                        format!("ryframe-owner:v1:{SCOPE}:object-storage:{}", request.bucket);
                    if self.fault == Fault::FinalOwner
                        && destination
                            .file_name()
                            .unwrap()
                            .to_string_lossy()
                            .starts_with("owner-final")
                    {
                        body.replace_range(..1, "x");
                    }
                    fs::write(destination, body.as_bytes())?;
                    return Ok(json!({"ContentLength":body.len()}));
                }
                assert!(!self.finalizing, "收尾不得下载业务对象");
                fs::write(destination, b"data")?;
                Ok(raw())
            }
            _ => panic!("对象收尾只允许 HEAD、完整列表和 owner GET"),
        }
    }
}

#[test]
fn final_heads_keep_four_way_batches_plan_order_and_five_exact_scopes() {
    let fixture = Fixture::new(9);
    let fake = Fake::new(9, Fault::None, true);
    let receipt = verify(fixture.request(), &fake, &|| Ok(())).unwrap();
    assert_eq!(fake.peak.load(Ordering::SeqCst), 4);
    assert_eq!(fake.active.load(Ordering::SeqCst), 0);
    assert_eq!(
        receipt["objects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["resource"]["key"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>(),
        (0..9).map(key).collect::<Vec<_>>()
    );
    assert_eq!(receipt["ownership_before"].as_array().unwrap().len(), 5);
    assert_eq!(receipt["ownership_after"].as_array().unwrap().len(), 5);
    assert_eq!(receipt["ownership_final"].as_array().unwrap().len(), 5);
    assert_eq!(receipt["keys"].as_object().unwrap().len(), 5);
    assert_eq!(receipt["keys"][BUCKET].as_array().unwrap().len(), 10);
    assert_eq!(receipt["business_downloads"], 0);
    assert_eq!(receipt["remote_writes"], 0);
    assert_eq!(fixture.result("verified.json"), receipt);
    assert_eq!(fixture.result("head-8.json"), raw());
}

#[test]
fn changed_raw_response_or_request_failure_drains_batch_without_retries() {
    for fault in [Fault::RawHead, Fault::HeadError] {
        let fixture = Fixture::new(5);
        let fake = Fake::new(5, fault, true);
        assert!(verify(fixture.request(), &fake, &|| Ok(())).is_err());
        assert_eq!(fake.active.load(Ordering::SeqCst), 0);
        assert_eq!(
            fake.heads
                .lock()
                .unwrap()
                .iter()
                .copied()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([0, 1, 2, 3])
        );
        assert!(fake.lists.lock().unwrap().is_empty());
        assert!(!fixture.directory.join("verified.json").exists());
        assert_eq!(fixture.result("failure.json")["stage"], "heads");
        let diagnostic = fixture.result("head-0.diagnostic.json");
        assert!(!diagnostic.to_string().contains("private-fixture-secret"));
        if fault == Fault::RawHead {
            assert_eq!(fixture.result("head-0.json")["AcceptRanges"], "bytes");
        } else {
            assert_eq!(diagnostic["returned"], false);
        }
    }
}

#[test]
fn listing_owner_and_prior_capture_drift_fail_closed() {
    for (fault, stage) in [
        (Fault::ExtraKey, "complete_listing"),
        (Fault::FinalOwner, "owners_final"),
    ] {
        let fixture = Fixture::new(1);
        let fake = Fake::new(1, fault, true);
        assert!(verify(fixture.request(), &fake, &|| Ok(())).is_err());
        assert_eq!(fixture.result("failure.json")["stage"], stage);
        assert_eq!(fake.lists.lock().unwrap().len(), 5);
    }
    let fixture = Fixture::new(1);
    fs::write(fixture.work.join("capture-0/object.bin"), b"evil").unwrap();
    let fake = Fake::new(1, Fault::None, true);
    assert!(verify(fixture.request(), &fake, &|| Ok(())).is_err());
    assert!(fake.heads.lock().unwrap().is_empty());
    assert!(!fixture.directory.join("verified.json").exists());
}

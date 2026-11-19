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
const NON_BUSINESS_BUCKET: &str = "config-packages";
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

    fn assert_head_evidence(&self, count: usize, checkpoint_passed: bool) {
        let mut expected = BTreeSet::new();
        for index in 0..count {
            let head = format!("head-{index}.json");
            let diagnostic = format!("head-{index}.diagnostic.json");
            assert_eq!(self.result(&head), raw());
            assert_eq!(
                self.result(&diagnostic),
                json!({"operation":"head-object","returned":true,"checkpoint_passed":checkpoint_passed})
            );
            expected.extend([head, diagnostic]);
        }
        let actual = fs::read_dir(&self.directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.starts_with("head-"))
            .collect::<BTreeSet<_>>();
        assert_eq!(actual, expected);
    }
}

fn key(index: usize) -> String {
    format!("{SCOPE}/item-{index}")
}

fn raw() -> Value {
    json!({"ContentType":"text/plain","ContentLength":4,"Metadata":{},"ETag":ETAG,
        "LastModified":"2026-11-09T00:00:00+00:00"})
}

fn owner_body(bucket: &str) -> String {
    format!("ryframe-owner:v1:{SCOPE}:object-storage:{bucket}")
}

fn record_observation_failure(path: &Path) -> Result<()> {
    document::write_new(path, &json!({"status":"object_observation_failed"}))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fault {
    None,
    RawHead,
    HeadError,
    ExtraKey,
    OwnerBoundary(&'static str),
    FinalOwner,
}

struct Fake {
    objects: usize,
    fault: Fault,
    finalizing: bool,
    heads: Mutex<Vec<usize>>,
    finished: Mutex<Vec<usize>>,
    lists: Mutex<Vec<String>>,
    owners: Mutex<Vec<String>>,
    gate: Condvar,
    active: AtomicUsize,
    peak: AtomicUsize,
    configuration_change: Option<(PathBuf, Vec<u8>)>,
    observation_failure_in_listing: Option<PathBuf>,
}

impl Fake {
    fn new(objects: usize, fault: Fault, finalizing: bool) -> Self {
        Self {
            objects,
            fault,
            finalizing,
            heads: Mutex::new(Vec::new()),
            finished: Mutex::new(Vec::new()),
            lists: Mutex::new(Vec::new()),
            owners: Mutex::new(Vec::new()),
            gate: Condvar::new(),
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            configuration_change: None,
            observation_failure_in_listing: None,
        }
    }

    fn assert_head_calls(&self, count: usize) {
        let expected = (0..count).collect::<Vec<_>>();
        for calls in [&self.heads, &self.finished] {
            let mut actual = calls.lock().unwrap().clone();
            actual.sort_unstable();
            assert_eq!(actual, expected);
        }
        assert_eq!(self.active.load(Ordering::SeqCst), 0);
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
                assert_eq!(target, key(index));
                let mut heads = self.heads.lock().unwrap();
                heads.push(index);
                let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.peak.fetch_max(active, Ordering::SeqCst);
                let end = (index / HEAD_WORKERS + 1) * HEAD_WORKERS;
                // 第四次调用进入传输后才改写，随后释放整批。
                let configuration_write = match &self.configuration_change {
                    Some((path, replacement))
                        if end == HEAD_WORKERS && heads.len() == HEAD_WORKERS =>
                    {
                        fs::write(path, replacement)
                    }
                    _ => Ok(()),
                };
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
                self.finished.lock().unwrap().push(index);
                self.active.fetch_sub(1, Ordering::SeqCst);
                configuration_write?;
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
                    if let Some(marker) = &self.observation_failure_in_listing {
                        self.assert_head_calls(self.objects);
                        record_observation_failure(marker)?;
                    }
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
                    let corrupt = match self.fault {
                        Fault::OwnerBoundary(phase) => {
                            request.bucket == NON_BUSINESS_BUCKET && name.starts_with(phase)
                        }
                        Fault::FinalOwner => name.starts_with("owner-final"),
                        _ => false,
                    };
                    if self.finalizing {
                        self.owners.lock().unwrap().push(name.into_owned());
                    }
                    let mut body = owner_body(&request.bucket);
                    if corrupt {
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
    fake.assert_head_calls(9);
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
    fixture.assert_head_evidence(9, true);
}

#[test]
fn changed_raw_response_or_request_failure_drains_batch_without_retries() {
    for fault in [Fault::RawHead, Fault::HeadError] {
        let fixture = Fixture::new(5);
        let fake = Fake::new(5, fault, true);
        assert!(verify(fixture.request(), &fake, &|| Ok(())).is_err());
        fake.assert_head_calls(HEAD_WORKERS);
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
fn changed_configuration_while_heads_are_in_flight_drains_without_publication() {
    let fixture = Fixture::new(5);
    let path = fixture
        .temp
        .path()
        .join(".local-tests/config/app.test.toml");
    let original = fs::read_to_string(&path).unwrap();
    let replacement = original.replace("fixture-private-secret", "fixture-private-rotate");
    assert_eq!(original.len(), replacement.len());
    assert!(original != replacement);
    let check_bound_config =
        || app_configuration::public_configuration(fixture.temp.path(), &fixture.configuration);
    assert!(check_bound_config().is_ok());
    let fake = Fake {
        configuration_change: Some((path, replacement.into_bytes())),
        ..Fake::new(5, Fault::None, true)
    };

    assert!(verify(fixture.request(), &fake, &|| Ok(())).is_err());

    let (path, changed) = fake.configuration_change.as_ref().unwrap();
    assert!(fs::read(path).unwrap().as_slice() == changed.as_slice());
    assert!(check_bound_config().is_err());
    assert_eq!(fake.peak.load(Ordering::SeqCst), HEAD_WORKERS);
    fake.assert_head_calls(HEAD_WORKERS);
    let owners = fake.owners.lock().unwrap();
    assert_eq!(owners.len(), BUCKETS.len());
    assert!(owners.iter().all(|name| name.starts_with("owner-before-")));
    assert!(fake.lists.lock().unwrap().is_empty());
    fixture.assert_head_evidence(HEAD_WORKERS, false);
    let failure = fixture.result("failure.json");
    assert_eq!(failure["stage"], "heads");
    assert_eq!(failure["remote_writes"], 0);
    assert!(!fixture.directory.join("verified.json").exists());
}

#[test]
fn rejects_other_bucket_owner_drift_at_both_head_boundaries() {
    for (phase, stage, head_count, owner_reads) in [
        ("owner-before", "owners_before", 0, 1),
        ("owner-after", "owners_after", 5, 2),
    ] {
        let fixture = Fixture::new(5);
        let fake = Fake::new(5, Fault::OwnerBoundary(phase), true);
        assert!(verify(fixture.request(), &fake, &|| Ok(())).is_err());
        fake.assert_head_calls(head_count);
        let owner = fake.owners.lock().unwrap();
        let suffix = format!("-{NON_BUSINESS_BUCKET}.bin");
        assert_eq!(
            owner
                .iter()
                .filter(|name| name.ends_with(suffix.as_str()))
                .count(),
            owner_reads
        );
        assert!(fake.lists.lock().unwrap().is_empty());
        fixture.assert_head_evidence(head_count, true);
        let stage_file = format!("{phase}-{NON_BUSINESS_BUCKET}");
        let expected = owner_body(NON_BUSINESS_BUCKET);
        let changed = format!("x{}", &expected[1..]);
        assert_eq!(
            fs::read(fixture.directory.join(format!("{stage_file}.bin"))).unwrap(),
            changed.as_bytes()
        );
        assert_eq!(
            fixture.result(&format!("{stage_file}.json")),
            json!({"ContentLength":expected.len()})
        );
        assert_eq!(
            fixture.result(&format!("{stage_file}.diagnostic.json")),
            json!({"operation":"get-object","returned":true,"checkpoint_passed":true})
        );
        let failure = fixture.result("failure.json");
        assert_eq!(failure["stage"], stage);
        assert_eq!(failure["remote_writes"], 0);
        assert!(!fixture.directory.join("verified.json").exists());
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

#[test]
fn late_observation_failure_blocks_success_before_and_after_publication() {
    for during_publication in [false, true] {
        let fixture = Fixture::new(5);
        let captured = fixture.work.join("capture-0");
        let marker = captured.join("failure.json");
        let original = ["observation.json", "capture.json", "object.bin"].map(|name| {
            let path = captured.join(name);
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        });
        let verified = fixture.directory.join("verified.json");
        let fake = Fake {
            observation_failure_in_listing: (!during_publication).then(|| marker.clone()),
            ..Fake::new(5, Fault::None, true)
        };
        let result = verify(fixture.request(), &fake, &|| {
            if during_publication && verified.is_file() {
                record_observation_failure(&marker)?;
            }
            Ok(())
        });

        assert!(result.is_err());
        fake.assert_head_calls(5);
        fixture.assert_head_evidence(5, true);
        assert_eq!(fake.lists.lock().unwrap().len(), BUCKETS.len());
        assert_eq!(fake.owners.lock().unwrap().len(), 3 * BUCKETS.len());
        for (path, bytes) in original {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
        assert_eq!(
            document::read(&marker).unwrap()["status"],
            "object_observation_failed"
        );
        let failure = fixture.result("failure.json");
        assert_eq!(failure["status"], "target_heads_failed");
        assert_eq!(failure["remote_writes"], 0);
        if during_publication {
            assert_eq!(failure["stage"], "publish");
            assert_eq!(
                fixture.result("verified.json")["status"],
                "target_object_heads_verified"
            );
        } else {
            assert_eq!(failure["stage"], "binding_after");
            assert!(!verified.exists());
        }
    }
}

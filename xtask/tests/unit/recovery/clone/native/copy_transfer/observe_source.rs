use std::{
    sync::{
        Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use super::*;
use crate::recovery::clone::native::export::{
    frozen::FrozenExport,
    transport::{CommandOutput, CommandSpec},
};

const SCOPE: &str = "fixture-source";
const ENDPOINT: &str = "http://127.0.0.1:29600";
const REGION: &str = "us-east-1";
const BUCKET: &str = "uploads";
const ETAG: &str = "\"fixture-etag\"";
const VERSION: &str = "fixture-version";

struct Fixture {
    root: tempfile::TempDir,
    work: std::path::PathBuf,
    directory: std::path::PathBuf,
    configuration: Value,
    export: Value,
    captures: BTreeMap<String, BTreeMap<String, Value>>,
}

impl Fixture {
    fn new(objects: usize) -> Self {
        let root = tempfile::tempdir_in(crate::workspace::root_dir()).unwrap();
        let config = root.path().join(".local-tests/config");
        fs::create_dir_all(&config).unwrap();
        fs::write(config.join("app.test.toml"), format!(
            "[object_storage]\nendpoint={ENDPOINT:?}\nregion={REGION:?}\naccess_key=\"private-fixture-access\"\nsecret_key=\"private-fixture-secret\"\n"
        )).unwrap();
        let configuration =
            app_configuration::configuration_value(root.path(), &config, "test").unwrap();
        let work = root.path().join(".local-tests/work");
        fs::create_dir(&work).unwrap();
        let directory = work.join("observation");
        let export_path = work.join("export.json");
        document::write_new(&export_path, &json!({"kind":"devex-clone-source-export"})).unwrap();
        let export = common::binding(&export_path).unwrap();
        let mut captures = BUCKETS
            .iter()
            .map(|&bucket| (bucket.to_owned(), BTreeMap::new()))
            .collect::<BTreeMap<_, _>>();
        let raw = metadata();
        let parsed = inspect_object(&raw).unwrap();
        for index in 0..objects {
            captures.get_mut(BUCKET).unwrap().insert(
                key(index),
                json!({
                    "identity":parsed["identity"],"metadata":parsed["metadata"],
                    "response_sha256":{"head_before":digest::plan_hash(&raw).unwrap()}
                }),
            );
        }
        Self {
            root,
            work,
            directory,
            configuration,
            export,
            captures,
        }
    }

    fn request(&self) -> ObserveSource<'_> {
        ObserveSource {
            backend: self.root.path(),
            work: &self.work,
            directory: &self.directory,
            export: &self.export,
            captures: &self.captures,
            scope: SCOPE,
            endpoint: ENDPOINT,
            region: REGION,
            configuration: &self.configuration,
        }
    }

    fn proof(&self, name: &str) -> Value {
        document::read(&self.directory.join(name)).unwrap()
    }
}

fn key(index: usize) -> String {
    format!("{SCOPE}/对象-{index}.bin")
}

fn metadata() -> Value {
    json!({"ContentType":"text/plain","ContentLength":4,"Metadata":{},"ETag":ETAG,
        "VersionId":VERSION,"LastModified":"2026-11-09T00:00:00+00:00"})
}

#[derive(Debug, PartialEq, Eq)]
enum Call {
    Owner(String),
    List(String, Option<String>),
    Head(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fault {
    None,
    Head404,
    HeadRawChanged,
    RepeatedCursor,
    ExtraAfter,
}

const HEAD_BATCH_SIZE: usize = 4;

#[derive(Default)]
struct HeadBatches<'a> {
    progress: Mutex<HeadProgress>,
    arrivals: Condvar,
    on_first_batch: Option<&'a (dyn Fn() -> Result<()> + Sync)>,
}

#[derive(Default)]
struct HeadProgress {
    started: BTreeSet<usize>,
    completed: usize,
    simultaneous: Vec<usize>,
    failed: bool,
}

impl HeadBatches<'_> {
    fn rendezvous(&self, fixture: &Fixture, key: &str) -> Result<()> {
        let keys = &fixture.captures[BUCKET];
        let index = keys.keys().position(|expected| expected.as_str() == key);
        let mut progress = self.progress.lock().unwrap();
        let start = progress.completed / HEAD_BATCH_SIZE * HEAD_BATCH_SIZE;
        let end = (start + HEAD_BATCH_SIZE).min(keys.len());
        let selected = index.filter(|index| (start..end).contains(index));
        let previous_batch_recorded = (0..start).all(|index| {
            fixture
                .directory
                .join(format!("head-{index}.diagnostic.json"))
                .is_file()
        });
        if progress.failed
            || !previous_batch_recorded
            || !selected.is_some_and(|index| progress.started.insert(index))
        {
            progress.failed = true;
            self.arrivals.notify_all();
            return Err("HEAD 与本批登记键不同、重复或前批证据尚未完成".into());
        }
        if progress.started.len() == end {
            if start == 0
                && let Some(on_first_batch) = self.on_first_batch
                && let Err(error) = on_first_batch()
            {
                progress.failed = true;
                self.arrivals.notify_all();
                return Err(error);
            }
            let active = end - progress.completed;
            progress.simultaneous.push(active);
            self.arrivals.notify_all();
        }
        let (mut progress, _) = self
            .arrivals
            .wait_timeout_while(progress, Duration::from_secs(10), |progress| {
                !progress.failed && progress.started.len() < end
            })
            .unwrap();
        if progress.failed || progress.started.len() < end {
            progress.failed = true;
            self.arrivals.notify_all();
            return Err("本批 HEAD 未在时限内同时到达".into());
        }
        progress.completed += 1;
        Ok(())
    }
}

struct FakeS3<'a> {
    fixture: &'a Fixture,
    fault: Fault,
    calls: Mutex<Vec<Call>>,
    uploads_pass: AtomicUsize,
    active_heads: AtomicUsize,
    maximum_heads: AtomicUsize,
    head_batches: Option<HeadBatches<'a>>,
}

impl<'a> FakeS3<'a> {
    fn new(fixture: &'a Fixture, fault: Fault) -> Self {
        Self {
            fixture,
            fault,
            calls: Mutex::new(Vec::new()),
            uploads_pass: AtomicUsize::new(0),
            active_heads: AtomicUsize::new(0),
            maximum_heads: AtomicUsize::new(0),
            head_batches: None,
        }
    }

    fn record(&self, call: Call) {
        self.calls.lock().unwrap().push(call);
    }

    fn list(&self, bucket: String, continuation: Option<String>) -> Value {
        self.record(Call::List(bucket.clone(), continuation.clone()));
        let owner = json!({"Key":format!("{SCOPE}/.ryframe-owner")});
        if bucket != BUCKET {
            assert_eq!(continuation, None);
            return json!({"IsTruncated":false,"Contents":[owner]});
        }
        if continuation.is_none() {
            self.uploads_pass.fetch_add(1, Ordering::SeqCst);
            return json!({"IsTruncated":true,"Contents":[owner],"NextContinuationToken":"page+2"});
        }
        assert_eq!(continuation.as_deref(), Some("page+2"));
        if self.fault == Fault::RepeatedCursor {
            return json!({"IsTruncated":true,"Contents":[],"NextContinuationToken":"page+2"});
        }
        let mut contents = self.fixture.captures[BUCKET]
            .keys()
            .map(|key| json!({"Key":key}))
            .collect::<Vec<_>>();
        if self.fault == Fault::ExtraAfter && self.uploads_pass.load(Ordering::SeqCst) == 2 {
            contents.push(json!({"Key":format!("{SCOPE}/unexpected")}));
        }
        json!({"IsTruncated":false,"Contents":contents})
    }
}

impl Transport for FakeS3<'_> {
    fn run(&self, _: CommandSpec) -> Result<CommandOutput> {
        panic!("源对象观察不得启动外部命令");
    }

    fn s3(&self, request: Request) -> Result<Value> {
        match request.operation {
            Operation::Get {
                key,
                if_match,
                version,
                destination,
                max_bytes,
            } => {
                assert_eq!(key, format!("{SCOPE}/.ryframe-owner"), "禁止业务 GET");
                assert_eq!((if_match, version, max_bytes), (None, None, 4096));
                assert_eq!(destination.parent(), Some(self.fixture.directory.as_path()));
                self.record(Call::Owner(request.bucket.clone()));
                let owner = format!("ryframe-owner:v1:{SCOPE}:object-storage:{}", request.bucket);
                fs::write(destination, &owner)?;
                Ok(json!({"ContentLength":owner.len()}))
            }
            Operation::Head {
                key,
                if_match,
                version,
            } => {
                assert_eq!(if_match.as_deref(), Some(ETAG));
                assert_eq!(version.as_deref(), Some(VERSION));
                assert_eq!(request.bucket, BUCKET);
                let active = self.active_heads.fetch_add(1, Ordering::SeqCst) + 1;
                self.maximum_heads.fetch_max(active, Ordering::SeqCst);
                self.record(Call::Head(key.clone()));
                let checked = self
                    .head_batches
                    .as_ref()
                    .map_or(Ok(()), |batches| batches.rendezvous(self.fixture, &key));
                self.active_heads.fetch_sub(1, Ordering::SeqCst);
                checked?;
                if self.fault == Fault::Head404 && key == self::key(0) {
                    return Ok(json!({"status":"not_found","http_status":404,
                        "remote_write_may_have_occurred":false}));
                }
                let mut raw = metadata();
                if self.fault == Fault::HeadRawChanged {
                    // 可选观察字段不改变解析后的身份或复制元数据，但必须改变原响应摘要。
                    raw["StorageClass"] = json!("STANDARD");
                }
                Ok(raw)
            }
            Operation::List {
                prefix,
                continuation,
            } => {
                assert_eq!(prefix, format!("{SCOPE}/"));
                Ok(self.list(request.bucket, continuation))
            }
            _ => panic!("源对象观察不得请求写入或其他操作"),
        }
    }
}

#[test]
fn single_object_uses_owner_head_owner_and_keeps_legacy_digest() {
    let fixture = Fixture::new(1);
    let fake = FakeS3::new(&fixture, Fault::None);
    let checks = AtomicUsize::new(0);
    let key = key(0);
    let result = observe(
        fixture.request(),
        Selection::One {
            bucket: BUCKET,
            key: &key,
        },
        &fake,
        &|| {
            checks.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(
        *fake.calls.lock().unwrap(),
        [
            Call::Owner(BUCKET.into()),
            Call::Head(key),
            Call::Owner(BUCKET.into()),
        ]
    );
    assert_eq!(checks.load(Ordering::SeqCst), 9);
    assert_eq!(result, fixture.proof("observation.json"));
    assert_eq!(result["whole_source"], false);
    assert_eq!(result["business_get_requests"], 0);
    assert_eq!(result["business_bytes_downloaded"], 0);
    assert_eq!(
        result["observed"][0]["head_sha256"],
        "64d9586b88e22edca51517df6e5652c64a53191994da885e41105b3af944d9df"
    );
    assert_eq!(
        result["identity_sha256"],
        "4351d962faa76dfbe485a6c321d8c6fb9c492dd667332b0ce2f80320e72d7d53"
    );
    assert_eq!(fixture.proof("head-0.json"), metadata());
    assert!(
        !fixture
            .proof("intent.json")
            .to_string()
            .contains("private-fixture-secret")
    );
}

#[test]
fn all_five_buckets_are_paginated_before_and_after_sorted_heads() {
    let fixture = Fixture::new(9);
    let fake = FakeS3 {
        head_batches: Some(HeadBatches::default()),
        ..FakeS3::new(&fixture, Fault::None)
    };
    let result = observe(
        fixture.request(),
        Selection::All {
            verify_proofs: &|| Ok(()),
        },
        &fake,
        &|| Ok(()),
    )
    .unwrap();
    let calls = fake.calls.lock().unwrap();
    let names = fixture.captures.keys().cloned().collect::<Vec<_>>();
    let owners = names.iter().cloned().map(Call::Owner).collect::<Vec<_>>();
    let lists = names
        .iter()
        .cloned()
        .map(|bucket| Call::List(bucket, None))
        .chain([Call::List(BUCKET.into(), Some("page+2".into()))])
        .collect::<Vec<_>>();
    assert_eq!(&calls[..5], owners.as_slice());
    assert_eq!(&calls[5..11], lists.as_slice());
    let keys = fixture.captures[BUCKET].keys().collect::<Vec<_>>();
    for (actual, expected) in calls[11..20]
        .chunks(HEAD_BATCH_SIZE)
        .zip(keys.chunks(HEAD_BATCH_SIZE))
    {
        let actual = actual
            .iter()
            .map(|call| match call {
                Call::Head(key) => key,
                _ => panic!("HEAD 批次中出现其他请求"),
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(actual, expected.iter().copied().collect::<BTreeSet<_>>());
    }
    assert_eq!(&calls[20..26], lists.as_slice());
    assert_eq!(&calls[26..], owners.as_slice());
    let progress = fake.head_batches.as_ref().unwrap().progress.lock().unwrap();
    assert_eq!(progress.simultaneous, [4, 4, 1]);
    assert_eq!(progress.completed, keys.len());
    assert_eq!(fake.maximum_heads.load(Ordering::SeqCst), HEAD_BATCH_SIZE);
    assert_eq!(result["whole_source"], true);
    assert_eq!(
        result["observed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["key"].as_str().unwrap())
            .collect::<Vec<_>>(),
        keys.iter().map(|key| key.as_str()).collect::<Vec<_>>()
    );
    for index in 0..keys.len() {
        assert_eq!(fixture.proof(&format!("head-{index}.json")), metadata());
        assert_eq!(
            fixture.proof(&format!("head-{index}.diagnostic.json")),
            json!({"operation":"head-object","returned":true,"checkpoint_passed":true})
        );
    }
    let head_receipts = fs::read_dir(&fixture.directory)
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("head-")
        })
        .count();
    assert_eq!(head_receipts, 18);
    assert_eq!(result["business_get_requests"], 0);
    assert_eq!(result["business_bytes_downloaded"], 0);
    assert_eq!(result["source_body_sha_recomputed"], false);
    assert_eq!(result["remote_writes"], 0);
    assert_eq!(
        fixture.proof("list-before-uploads-0.json")["NextContinuationToken"],
        "page+2"
    );
    assert!(
        fixture
            .directory
            .join("owner-after-config-packages.bin")
            .exists()
    );
}

#[test]
fn unexpected_head_status_or_raw_change_fails_without_retry() {
    for fault in [Fault::Head404, Fault::HeadRawChanged] {
        let fixture = Fixture::new(1);
        let fake = FakeS3::new(&fixture, fault);
        let outcome = observe(
            fixture.request(),
            Selection::One {
                bucket: BUCKET,
                key: &key(0),
            },
            &fake,
            &|| Ok(()),
        );
        assert!(outcome.is_err());
        assert_eq!(fake.calls.lock().unwrap().len(), 2);
        assert_eq!(fixture.proof("failure.json")["stage"], "heads");
        assert!(fixture.directory.join("head-0.json").exists());
        assert!(!fixture.directory.join("observation.json").exists());
    }
}

#[test]
fn failed_head_batch_drains_and_never_launches_the_next_batch() {
    let fixture = Fixture::new(6);
    let fake = FakeS3::new(&fixture, Fault::Head404);
    assert!(
        observe(
            fixture.request(),
            Selection::All {
                verify_proofs: &|| Ok(()),
            },
            &fake,
            &|| Ok(()),
        )
        .is_err()
    );
    let calls = fake.calls.lock().unwrap();
    assert_eq!(calls.len(), 11 + HEAD_WORKERS);
    assert_eq!(fake.active_heads.load(Ordering::SeqCst), 0);
    for index in 0..HEAD_WORKERS {
        assert!(
            fixture
                .directory
                .join(format!("head-{index}.json"))
                .exists()
        );
    }
    assert_eq!(fixture.proof("failure.json")["stage"], "heads");
}

#[test]
fn changed_source_proof_during_parallel_heads_blocks_publication() {
    let fixture = Fixture::new(9);
    let schema = fixture.work.join("schema-after.json");
    fs::write(&schema, b"{}\n").unwrap();
    let original = digest::file(&schema).unwrap();
    let frozen = FrozenExport {
        request: Value::Null,
        generation: Value::Null,
        inventory: Value::Null,
        captures: BTreeMap::new(),
        proof_files: json!({
            (manifest::path_text(&schema, "测试源证明").unwrap()):
                {"bytes":original.bytes,"sha256":original.sha256}
        }),
    };
    let verify_proofs = || frozen.check_proofs(fixture.root.path(), &fixture.export);
    verify_proofs().unwrap();
    let changed = b"{\"schema\":\"private-proof-marker\"}\n";
    let drift = || {
        fs::write(&schema, changed)?;
        Ok(())
    };
    let fake = FakeS3 {
        head_batches: Some(HeadBatches {
            on_first_batch: Some(&drift),
            ..HeadBatches::default()
        }),
        ..FakeS3::new(&fixture, Fault::None)
    };

    let error = observe(
        fixture.request(),
        Selection::All {
            verify_proofs: &verify_proofs,
        },
        &fake,
        &|| Ok(()),
    )
    .unwrap_err();

    let progress = fake.head_batches.as_ref().unwrap().progress.lock().unwrap();
    assert_eq!(progress.simultaneous.first(), Some(&HEAD_BATCH_SIZE));
    assert!(progress.completed >= HEAD_BATCH_SIZE);
    assert_eq!(fake.active_heads.load(Ordering::SeqCst), 0);
    for index in 0..HEAD_BATCH_SIZE {
        assert_eq!(fixture.proof(&format!("head-{index}.json")), metadata());
        assert_eq!(
            fixture.proof(&format!("head-{index}.diagnostic.json")),
            json!({"operation":"head-object","returned":true,"checkpoint_passed":true})
        );
    }
    assert_eq!(fs::read(&schema).unwrap().as_slice(), changed);
    let failure = fixture.proof("failure.json");
    assert_eq!(failure["stage"], "binding_after");
    assert_eq!(failure["remote_writes"], 0);
    assert!(!fixture.directory.join("observation.json").exists());
    assert!(!error.to_string().contains("private-proof-marker"));
    assert!(!failure.to_string().contains("private-proof-marker"));
}

#[test]
fn duplicate_cursor_and_changed_final_key_set_fail_with_raw_proofs() {
    for (fault, stage, name) in [
        (
            Fault::RepeatedCursor,
            "complete_listing_before",
            "list-before-uploads-1.json",
        ),
        (
            Fault::ExtraAfter,
            "complete_listing_after",
            "list-after-uploads-1.json",
        ),
    ] {
        let fixture = Fixture::new(1);
        let fake = FakeS3::new(&fixture, fault);
        assert!(
            observe(
                fixture.request(),
                Selection::All {
                    verify_proofs: &|| Ok(()),
                },
                &fake,
                &|| Ok(()),
            )
            .is_err()
        );
        assert_eq!(fixture.proof("failure.json")["stage"], stage);
        assert!(fixture.directory.join(name).exists());
        assert!(!fixture.directory.join("observation.json").exists());
    }
}

#[test]
fn checkpoint_failure_after_head_preserves_response_and_blocks_next_request() {
    let fixture = Fixture::new(1);
    let fake = FakeS3::new(&fixture, Fault::None);
    let checks = AtomicUsize::new(0);
    let outcome = observe(
        fixture.request(),
        Selection::One {
            bucket: BUCKET,
            key: &key(0),
        },
        &fake,
        &|| {
            if checks.fetch_add(1, Ordering::SeqCst) == 4 {
                return Err("private-fixture-secret".into());
            }
            Ok(())
        },
    );
    assert!(
        !outcome
            .unwrap_err()
            .to_string()
            .contains("private-fixture-secret")
    );
    assert_eq!(fake.calls.lock().unwrap().len(), 2);
    assert_eq!(fixture.proof("head-0.json"), metadata());
    assert_eq!(
        fixture.proof("head-0.diagnostic.json")["checkpoint_passed"],
        false
    );
    assert_eq!(fixture.proof("failure.json")["stage"], "heads");
}

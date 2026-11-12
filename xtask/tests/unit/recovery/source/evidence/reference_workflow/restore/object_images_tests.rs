use std::{
    fs,
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use super::*;
use crate::{contract::sha256_hex, recovery::clone::native::s3::Operation};

const SCOPE: &str = "fixture-target";
const ENDPOINT: &str = "http://127.0.0.1:29600";
const REGION: &str = "us-east-1";
const KEY: &str = "fixture-target/tenant/item";
const MISSING_KEY: &str = "fixture-target/tenant/missing";
const ETAG: &str = "\"fixture-etag\"";
const BODY: &[u8] = b"retained full object";

struct Fixture {
    _temp: tempfile::TempDir,
    backend: PathBuf,
    directory: PathBuf,
    configuration: Value,
    target: Value,
    environment: BTreeMap<String, String>,
    manifest: Value,
    expected: Value,
    phase: Phase,
}

impl Fixture {
    fn new(phase: Phase) -> Self {
        let temp = tempfile::tempdir_in(crate::workspace::root_dir()).unwrap();
        let backend = crate::recovery::fixture_control::portable_path(
            &fs::canonicalize(temp.path()).unwrap(),
        );
        let local = backend.join(".local-tests");
        let config = local.join("config");
        fs::create_dir_all(&config).unwrap();
        fs::write(config.join("app.test.toml"), format!(
            "[object_storage]\nendpoint={ENDPOINT:?}\nregion={REGION:?}\naccess_key=\"fixture-access\"\nsecret_key=\"fixture-secret\"\n"
        )).unwrap();
        let configuration =
            app_configuration::configuration_value(&backend, &config, "test").unwrap();
        let target = json!({"config_dir":config,"target":{"scope_id":SCOPE,"s3":{
            "endpoint":ENDPOINT,"region":REGION,"access_key":"fixture-access","secret_key":"fixture-secret"}}});
        let directory = local.join("phase");
        fs::create_dir(&directory).unwrap();
        let (manifest, expected) = manifest_and_image(!matches!(phase, Phase::Before));
        Self {
            _temp: temp,
            backend,
            directory,
            configuration,
            target,
            manifest,
            expected,
            phase,
            environment: BTreeMap::from([("APP_ENV".into(), "test".into())]),
        }
    }

    fn with_missing_planned_object(mut self) -> Self {
        let bucket = self.manifest["objects"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|bucket| bucket["bucket"] == "uploads")
            .unwrap();
        bucket["entries"].as_array_mut().unwrap().push(json!({
            "key":"fixture-source/tenant/missing","bytes":BODY.len(),"sha256":sha256_hex(BODY)
        }));
        self.expected["uploads"]["keys"]
            .as_array_mut()
            .unwrap()
            .push(json!(MISSING_KEY));
        self
    }

    fn capture(
        &self,
        transport: &Fake,
        checkpoint: &(dyn Fn() -> Result<()> + Sync),
    ) -> Result<(Value, Vec<Value>)> {
        let storage = StorageInputs {
            backend: &self.backend,
            request: &self.target,
            environment: &self.environment,
            observation_dir: &self.directory,
        };
        capture(
            &Observe {
                storage: &storage,
                configuration_root: &self.backend,
                configuration: &self.configuration,
                directory: &self.directory,
                scope: SCOPE,
                endpoint: ENDPOINT,
                region: REGION,
                manifest: &self.manifest,
                expected: &self.expected,
                plan_sha256: &"a".repeat(64),
                generation_sha256: &"b".repeat(64),
            },
            self.phase,
            transport,
            checkpoint,
        )
    }
}

fn manifest_and_image(after: bool) -> (Value, Value) {
    let mut image = BTreeMap::new();
    let mut objects = Vec::new();
    for bucket in BUCKETS {
        let owner = owner(bucket);
        let mut keys = vec![format!("{SCOPE}/.ryframe-owner")];
        let mut entries = Vec::new();
        if after && bucket == "uploads" {
            keys.push(KEY.into());
            entries.push(json!({"key":"fixture-source/tenant/item","bytes":BODY.len(),"sha256":sha256_hex(BODY)}));
        }
        objects.push(json!({"bucket":bucket,"prefix":"fixture-source/","entries":entries}));
        image.insert(bucket, json!({"keys":keys,"owner":{"bytes":owner.len(),"sha256":sha256_hex(owner.as_bytes())}}));
    }
    (
        json!({"scope_id":"fixture-source","objects":objects}),
        json!(image),
    )
}

fn owner(bucket: &str) -> String {
    format!("ryframe-owner:v1:{SCOPE}:object-storage:{bucket}")
}

fn response(bytes: usize) -> Value {
    json!({"ContentType":"text/plain","ContentLength":bytes,"Metadata":{},"ETag":ETAG,
        "LastModified":"2026-11-09T00:00:00+00:00"})
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    None,
    InitialExtra,
    InitialOwnerMissing,
    ChangedOwner,
    IndependentHead,
    FinalListing,
    PlannedKeyAppears,
}

struct Fake {
    after: bool,
    fault: Fault,
    heads: AtomicUsize,
    downloads: AtomicUsize,
    lists: Mutex<BTreeMap<String, usize>>,
}

impl Fake {
    fn new(after: bool, fault: Fault) -> Self {
        Self {
            after,
            fault,
            heads: AtomicUsize::new(0),
            downloads: AtomicUsize::new(0),
            lists: Mutex::new(BTreeMap::new()),
        }
    }

    fn list(&self, bucket: &str) -> Value {
        let mut lists = self.lists.lock().unwrap();
        let count = lists.entry(bucket.to_owned()).or_default();
        *count += 1;
        let mut keys = vec![json!({"Key":format!("{SCOPE}/.ryframe-owner")})];
        if self.fault == Fault::InitialOwnerMissing && bucket == "imports" && *count == 1 {
            keys.clear();
        }
        if self.after && bucket == "uploads" {
            keys.push(json!({"Key":KEY}));
            if (self.fault == Fault::InitialExtra && *count == 1)
                || (self.fault == Fault::FinalListing && *count == 3)
            {
                keys.push(json!({"Key":format!("{SCOPE}/extra")}));
            }
            if self.fault == Fault::PlannedKeyAppears && *count == 3 {
                keys.push(json!({"Key":MISSING_KEY}));
            }
        }
        json!({"IsTruncated":false,"Contents":keys})
    }
}

impl Transport for Fake {
    fn run(&self, _: CommandSpec) -> Result<CommandOutput> {
        panic!("对象观察不得启动外部命令");
    }

    fn s3(&self, request: Request) -> Result<Value> {
        match request.operation {
            Operation::HeadBucket | Operation::Versioning => Ok(json!({})),
            Operation::List {
                prefix,
                continuation,
            } => {
                assert_eq!(prefix, format!("{SCOPE}/"));
                assert!(continuation.is_none());
                Ok(self.list(&request.bucket))
            }
            Operation::Head {
                key,
                if_match,
                version,
            } => {
                assert_eq!(key, KEY);
                assert!(version.is_none());
                let count = self.heads.fetch_add(1, Ordering::SeqCst);
                assert_eq!(if_match.as_deref(), (count > 0).then_some(ETAG));
                let mut result = response(BODY.len());
                if count == 2 && self.fault == Fault::IndependentHead {
                    result["AcceptRanges"] = json!("bytes");
                }
                Ok(result)
            }
            Operation::Get {
                key,
                if_match,
                version,
                destination,
                max_bytes,
            } => {
                assert!(version.is_none());
                if key == format!("{SCOPE}/.ryframe-owner") {
                    assert!(if_match.is_none());
                    assert_eq!(max_bytes, 4096);
                    let mut owner = owner(&request.bucket);
                    if self.fault == Fault::ChangedOwner && request.bucket == "exports" {
                        owner = owner.replace(SCOPE, "another-scope");
                    }
                    fs::write(destination, owner.as_bytes())?;
                    Ok(response(owner.len()))
                } else {
                    assert_eq!(key, KEY);
                    assert_eq!(if_match.as_deref(), Some(ETAG));
                    assert_eq!(max_bytes, BODY.len() as u64);
                    self.downloads.fetch_add(1, Ordering::SeqCst);
                    fs::write(destination, BODY)?;
                    Ok(response(BODY.len()))
                }
            }
            _ => panic!("对象观察不接受写操作"),
        }
    }
}

#[test]
fn before_uses_owner_only_and_per_read_checkpoint() {
    let fixture = Fixture::new(Phase::Before);
    let fake = Fake::new(false, Fault::None);
    let (image, evidence) = fixture.capture(&fake, &|| Ok(())).unwrap();
    assert_eq!(image, fixture.expected);
    assert!(evidence.is_empty());
    assert_eq!(fake.downloads.load(Ordering::SeqCst), 0);

    let fixture = Fixture::new(Phase::Before);
    let calls = AtomicUsize::new(0);
    let checkpoint = || {
        let call = calls.fetch_add(1, Ordering::SeqCst);
        if call == 2 {
            Err("前像原绑定已变化".into())
        } else {
            Ok(())
        }
    };
    assert!(
        fixture
            .capture(&Fake::new(false, Fault::None), &checkpoint)
            .is_err()
    );
    assert!(fixture.directory.read_dir().unwrap().count() > 0);
}

#[test]
fn captures_full_payload_and_binds_observation_and_independent_head_receipts() {
    let fixture = Fixture::new(Phase::After);
    let fake = Fake::new(true, Fault::None);
    let (image, evidence) = fixture.capture(&fake, &|| Ok(())).unwrap();
    assert_eq!(image, fixture.expected);
    assert_eq!(fake.downloads.load(Ordering::SeqCst), 1);
    assert_eq!(fake.heads.load(Ordering::SeqCst), 3);
    let observation = fixture.directory.join("object-uploads-0/observation.json");
    let final_head = fixture.directory.join("object-heads/verified.json");
    assert_eq!(
        evidence,
        [
            common::binding(&observation).unwrap(),
            common::binding(&final_head).unwrap()
        ]
    );
    assert_eq!(
        fs::read(observation.with_file_name("object.bin")).unwrap(),
        BODY
    );
    assert_eq!(
        document::read(&final_head).unwrap()["status"],
        "target_object_heads_verified"
    );
}

#[test]
fn unknown_initial_key_or_late_drift_cannot_return_success() {
    for (fault, downloads, verified) in [
        (Fault::InitialExtra, 0, false),
        (Fault::IndependentHead, 1, false),
        (Fault::FinalListing, 1, true),
    ] {
        let fixture = Fixture::new(Phase::After);
        let fake = Fake::new(true, fault);
        assert!(fixture.capture(&fake, &|| Ok(())).is_err());
        assert_eq!(fake.downloads.load(Ordering::SeqCst), downloads);
        assert_eq!(
            fixture
                .directory
                .join("object-heads/verified.json")
                .exists(),
            verified
        );
        if downloads == 1 {
            assert_eq!(
                fs::read(fixture.directory.join("object-uploads-0/object.bin")).unwrap(),
                BODY
            );
        }
    }
}

#[test]
fn failure_after_captures_all_five_buckets_with_zero_or_one_of_two_planned_objects() {
    for (present, count) in [(false, 0), (true, 1)] {
        let fixture = Fixture::new(Phase::FailureAfter).with_missing_planned_object();
        let fake = Fake::new(present, Fault::None);
        let (image, evidence) = fixture.capture(&fake, &|| Ok(())).unwrap();
        let (_, expected_actual) = manifest_and_image(present);
        assert_eq!(image, expected_actual);
        assert_ne!(image, fixture.expected);
        assert_eq!(fake.downloads.load(Ordering::SeqCst), count);
        assert_eq!(fake.heads.load(Ordering::SeqCst), count * 3);
        assert_eq!(
            *fake.lists.lock().unwrap(),
            BUCKETS
                .into_iter()
                .map(|bucket| (bucket.to_owned(), 3))
                .collect::<BTreeMap<_, _>>()
        );

        let final_head = fixture.directory.join("object-heads/verified.json");
        let observation = fixture.directory.join("object-uploads-0/observation.json");
        let mut bindings = Vec::new();
        if present {
            assert_eq!(
                fs::read(observation.with_file_name("object.bin")).unwrap(),
                BODY
            );
            assert_eq!(
                document::read(&observation).unwrap()["evidence"]["object.bin"],
                json!({"bytes":BODY.len(),"sha256":sha256_hex(BODY)})
            );
            bindings.push(common::binding(&observation).unwrap());
        } else {
            assert!(!observation.exists());
        }
        assert!(!fixture.directory.join("object-uploads-1").exists());
        bindings.push(common::binding(&final_head).unwrap());
        assert_eq!(evidence, bindings);
        let verified = document::read(&final_head).unwrap();
        let expected_keys = BUCKETS
            .into_iter()
            .map(|bucket| (bucket, &expected_actual[bucket]["keys"]))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(verified["keys"], json!(expected_keys));
        assert_eq!(verified["objects"].as_array().unwrap().len(), count);
        assert_eq!(verified["remote_writes"], 0);
    }
}

#[test]
fn successful_after_still_rejects_missing_planned_objects_before_business_reads() {
    let fixture = Fixture::new(Phase::After).with_missing_planned_object();
    let fake = Fake::new(true, Fault::None);
    assert!(fixture.capture(&fake, &|| Ok(())).is_err());
    assert_eq!(fake.heads.load(Ordering::SeqCst), 0);
    assert_eq!(fake.downloads.load(Ordering::SeqCst), 0);
    assert!(
        !fixture
            .directory
            .join("object-heads/verified.json")
            .exists()
    );
}

#[test]
fn failure_after_rejects_owner_changes_unknown_keys_and_observation_drift() {
    for (fault, downloads, verified) in [
        (Fault::InitialExtra, 0, false),
        (Fault::InitialOwnerMissing, 0, false),
        (Fault::ChangedOwner, 0, false),
        (Fault::IndependentHead, 1, false),
        (Fault::PlannedKeyAppears, 1, true),
    ] {
        let fixture = Fixture::new(Phase::FailureAfter).with_missing_planned_object();
        let fake = Fake::new(true, fault);
        assert!(fixture.capture(&fake, &|| Ok(())).is_err(), "{fault:?}");
        assert_eq!(
            fake.downloads.load(Ordering::SeqCst),
            downloads,
            "{fault:?}"
        );
        assert_eq!(
            fixture
                .directory
                .join("object-heads/verified.json")
                .exists(),
            verified,
            "{fault:?}"
        );
    }
}

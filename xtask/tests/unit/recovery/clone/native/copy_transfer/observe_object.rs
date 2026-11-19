use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
};

use super::*;
use crate::recovery::clone::native::{
    copy_transfer::images::{TargetObject, object_observed},
    export::transport::{CommandOutput, CommandSpec},
};

const SCOPE: &str = "fixture-target";
const FOREIGN_SCOPE: &str = "foreign-target";
const ENDPOINT: &str = "http://127.0.0.1:29600";
const REGION: &str = "us-east-1";
const BUCKET: &str = "uploads";
const KEY: &str = "fixture-target/file.txt";
const ETAG: &str = "\"fixture-etag\"";
const VERSION: &str = "fixture-version";

struct Fixture {
    temp: tempfile::TempDir,
    work: PathBuf,
    directory: PathBuf,
    configuration: Value,
    expected: Value,
    plan: String,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir_in(crate::workspace::root_dir()).unwrap();
        let config = temp.path().join(".local-tests/config");
        fs::create_dir_all(&config).unwrap();
        fs::write(
            config.join("app.test.toml"),
            format!(
                "[object_storage]\nendpoint = {ENDPOINT:?}\nregion = {REGION:?}\naccess_key = \"private-fixture-access\"\nsecret_key = \"private-fixture-secret\"\n"
            ),
        )
        .unwrap();
        let configuration =
            app_configuration::configuration_value(temp.path(), &config, "test").unwrap();
        let work = temp.path().join(".local-tests/work");
        fs::create_dir(&work).unwrap();
        let directory = work.join("observation");
        let expected = json!({"bytes":4,"sha256":hex::encode(Sha256::digest(b"data"))});
        Self {
            temp,
            work,
            directory,
            configuration,
            expected,
            plan: "a".repeat(64),
        }
    }

    fn request(&self) -> ObserveTarget<'_> {
        ObserveTarget {
            backend: self.temp.path(),
            configuration_root: self.temp.path(),
            work: &self.work,
            directory: &self.directory,
            plan_sha256: &self.plan,
            scope: SCOPE,
            endpoint: ENDPOINT,
            region: REGION,
            configuration: &self.configuration,
            bucket: BUCKET,
            key: KEY,
            expected: &self.expected,
            max_bytes: 4,
        }
    }

    fn proof(&self, name: &str) -> Value {
        document::read(&self.directory.join(name)).unwrap()
    }

    fn files(&self) -> BTreeMap<String, Vec<u8>> {
        fs::read_dir(&self.directory)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.file_name().into_string().unwrap(),
                    fs::read(entry.path()).unwrap(),
                )
            })
            .collect()
    }

    fn image(&self, observed: &Observed) -> Value {
        let item = json!({"bucket":BUCKET,"source_key":"fixture-source/file.txt","target_key":KEY,
            "artifact":{"file":"object.bin","bytes":4,"sha256":self.expected["sha256"]},"metadata":{}});
        object_observed(
            &self.plan,
            TargetObject {
                scope: SCOPE,
                endpoint: ENDPOINT,
                item: &item,
            },
            observed.image_evidence(),
        )
        .unwrap()
    }
}

enum Step {
    Owner,
    OwnerWrongScope,
    Head {
        conditional: bool,
        raw: Option<Value>,
    },
    Get {
        raw: Option<Value>,
    },
}

struct FakeS3 {
    steps: RefCell<VecDeque<Step>>,
    drift_during_get: Option<PathBuf>,
}

impl FakeS3 {
    fn new(steps: impl IntoIterator<Item = Step>) -> Self {
        Self {
            steps: RefCell::new(steps.into_iter().collect()),
            drift_during_get: None,
        }
    }

    fn exhausted(&self) {
        assert!(self.steps.borrow().is_empty());
    }
}

impl Transport for FakeS3 {
    fn run(&self, _: CommandSpec) -> Result<CommandOutput> {
        panic!("对象观察不得启动外部命令");
    }

    fn s3(&self, request: Request) -> Result<Value> {
        assert_eq!(request.bucket, BUCKET);
        let step = self
            .steps
            .borrow_mut()
            .pop_front()
            .expect("对象观察不得重试");
        match (request.operation, step) {
            (
                Operation::Get {
                    key,
                    if_match,
                    version,
                    destination,
                    max_bytes,
                },
                step @ (Step::Owner | Step::OwnerWrongScope),
            ) => {
                assert_eq!(key, format!("{SCOPE}/.ryframe-owner"));
                assert_eq!((if_match, version, max_bytes), (None, None, 4096));
                let owner = owner_marker(if matches!(step, Step::Owner) {
                    SCOPE
                } else {
                    FOREIGN_SCOPE
                });
                fs::write(destination, &owner)?;
                Ok(json!({"ContentLength":owner.len()}))
            }
            (
                Operation::Head {
                    key,
                    if_match,
                    version,
                },
                Step::Head { conditional, raw },
            ) => {
                assert_eq!(key, KEY);
                assert_eq!(if_match.as_deref(), conditional.then_some(ETAG));
                assert_eq!(version.as_deref(), conditional.then_some(VERSION));
                raw.ok_or_else(|| "transport-error private-fixture-secret".into())
            }
            (
                Operation::Get {
                    key,
                    if_match,
                    version,
                    destination,
                    max_bytes,
                },
                Step::Get { raw },
            ) => {
                assert_eq!(key, KEY);
                assert_eq!(if_match.as_deref(), Some(ETAG));
                assert_eq!(version.as_deref(), Some(VERSION));
                assert_eq!(max_bytes, 4);
                let raw = raw.ok_or("HTTP 404 private-fixture-secret")?;
                if raw.get("status").is_none() {
                    fs::write(destination, b"data")?;
                    if let Some(path) = &self.drift_during_get {
                        alter_evidence(path)?;
                    }
                }
                Ok(raw)
            }
            _ => panic!("目标观察顺序或请求类型错误"),
        }
    }
}

fn absent() -> Value {
    json!({"status":"not_found","http_status":404,"remote_write_may_have_occurred":false})
}

fn owner_marker(scope: &str) -> String {
    format!("ryframe-owner:v1:{scope}:object-storage:{BUCKET}")
}

fn metadata() -> Value {
    json!({"ContentType":"text/plain","ContentLength":4,"Metadata":{},"ETag":ETAG,
        "VersionId":VERSION,"LastModified":"2026-11-09T00:00:00+00:00"})
}

fn head(raw: Value) -> Step {
    Step::Head {
        conditional: false,
        raw: Some(raw),
    }
}

fn alter_evidence(path: &Path) -> Result<()> {
    let mut bytes = fs::read(path)?;
    assert_eq!(bytes.first(), Some(&b'{'));
    bytes[0] = b'[';
    fs::write(path, bytes)?;
    Ok(())
}

fn assert_failed(fixture: &Fixture, fake: &FakeS3, stage: &str) {
    let error = observe(fixture.request(), fake, &|| Ok(()))
        .err()
        .unwrap()
        .to_string();
    fake.exhausted();
    let failure = fixture.proof("failure.json");
    assert_eq!(failure["stage"], stage);
    assert_eq!(failure["remote_writes"], 0);
    assert!(!fixture.directory.join("capture.json").exists());
    assert!(!fixture.directory.join("observation.json").exists());
    assert!(!error.contains("private-fixture-secret"));
    assert!(!failure.to_string().contains("private-fixture-secret"));
}

#[test]
fn requires_two_explicit_head_404_responses_and_owner_bracket() {
    let fixture = Fixture::new();
    let fake = FakeS3::new([Step::Owner, head(absent()), Step::Owner, head(absent())]);
    let checkpoints = Cell::new(0);
    let observed = observe(fixture.request(), &fake, &|| {
        checkpoints.set(checkpoints.get() + 1);
        Ok(())
    })
    .unwrap();
    fake.exhausted();
    assert_eq!(checkpoints.get(), 10);
    assert_eq!(fixture.proof("head-before.json"), absent());
    assert_eq!(fixture.proof("head-after.json"), absent());
    assert_eq!(observed.receipt(), &fixture.proof("observation.json"));
    assert_eq!(observed.receipt()["status"], "object_absent");
    assert_eq!(
        observed.descriptor(),
        &common::binding(&fixture.directory.join("observation.json")).unwrap()
    );
    assert_eq!(fixture.image(&observed)["exists"], false);
}

#[test]
fn rejection_does_not_retry_or_start_later_requests() {
    for (steps, stage) in [
        (vec![Step::OwnerWrongScope], "owner-before"),
        (
            vec![Step::Owner, head(metadata()), Step::Get { raw: None }],
            "get",
        ),
        (
            vec![
                Step::Owner,
                head(metadata()),
                Step::Get {
                    raw: Some(absent()),
                },
            ],
            "get",
        ),
        (
            vec![
                Step::Owner,
                Step::Head {
                    conditional: false,
                    raw: None,
                },
            ],
            "head-before",
        ),
        (
            vec![
                Step::Owner,
                head(json!({"status":"not_found","http_status":404,
            "remote_write_may_have_occurred":false,"extra":true})),
            ],
            "head-before",
        ),
        (
            vec![Step::Owner, head(absent()), Step::Owner, head(metadata())],
            "head-after",
        ),
    ] {
        let fixture = Fixture::new();
        let fake = FakeS3::new(steps);
        assert_failed(&fixture, &fake, stage);
        assert!(
            fixture
                .directory
                .join(format!("{stage}.diagnostic.json"))
                .exists()
        );
    }
}

#[test]
fn captures_exact_version_and_bytes_and_preserves_get_header_difference() {
    let fixture = Fixture::new();
    let mut get = metadata();
    get["ContentType"] = json!("application/octet-stream");
    let fake = FakeS3::new(captured_steps(get.clone(), metadata(), Step::Owner));
    let observed = observe(fixture.request(), &fake, &|| Ok(())).unwrap();
    fake.exhausted();
    assert_eq!(observed.receipt()["status"], "object_present");
    assert_eq!(observed.receipt(), &fixture.proof("observation.json"));
    assert_eq!(
        observed.descriptor(),
        &common::binding(&fixture.directory.join("observation.json")).unwrap()
    );
    assert_eq!(
        observed.receipt()["capture"],
        common::binding(&fixture.directory.join("capture.json")).unwrap()
    );
    assert_eq!(fixture.proof("head-before.json"), metadata());
    assert_eq!(fixture.proof("get.json"), get);
    assert_eq!(fixture.proof("head-after.json"), metadata());
    let ObjectObservation::Captured(captured) = observed.image_evidence() else {
        panic!("存在对象必须返回完整采集");
    };
    assert_eq!(captured, &fixture.proof("capture.json"));
    assert_eq!(captured["identity"]["ETag"], ETAG);
    assert_eq!(captured["identity"]["VersionId"], VERSION);
    assert_eq!(captured["artifact"]["sha256"], fixture.expected["sha256"]);
    for phase in ["before", "after"] {
        let field = format!("ownership_{phase}");
        assert_eq!(observed.receipt()[&field], captured[&field]);
    }
    let image = fixture.image(&observed);
    assert_eq!(image["exists"], true);
    assert_eq!(
        image["metadata_sha256"],
        digest::plan_hash(&json!({"stored":captured["metadata"],
            "get_header_differences":captured["get_header_differences"]}))
        .unwrap()
    );
    assert_eq!(
        captured["get_header_differences"][0]["field"],
        "ContentType"
    );
    assert!(
        !fixture
            .proof("intent.json")
            .to_string()
            .contains("private-fixture-secret")
    );
    let files = fixture.files();
    assert_original_evidence(&files, observed.receipt());
    assert!(observe(fixture.request(), &fake, &|| Ok(())).is_err());
    fake.exhausted();
    assert_eq!(fixture.files(), files);
}

fn assert_original_evidence(files: &BTreeMap<String, Vec<u8>>, receipt: &Value) {
    let names = [
        "intent.json",
        "owner-before.json",
        "owner-before.diagnostic.json",
        "owner-before-uploads.bin",
        "head-before.json",
        "head-before.diagnostic.json",
        "get.json",
        "get.diagnostic.json",
        "object.bin",
        "head-after.json",
        "head-after.diagnostic.json",
        "owner-after.json",
        "owner-after.diagnostic.json",
        "owner-after-uploads.bin",
        "capture.json",
    ];
    let evidence = receipt["evidence"].as_object().unwrap();
    assert_eq!(evidence.len(), names.len());
    assert_eq!(files.len(), names.len() + 1);
    for name in names {
        let raw = &files[name];
        assert_eq!(
            evidence[name],
            json!({"bytes":raw.len(),"sha256":hex::encode(Sha256::digest(raw))}),
            "{name}"
        );
    }
    assert_eq!(files["object.bin"], b"data");
    let owner = owner_marker(SCOPE);
    for name in ["owner-before-uploads.bin", "owner-after-uploads.bin"] {
        assert_eq!(files[name], owner.as_bytes());
    }
}

#[test]
fn rejects_preexisting_head_without_contacting_s3_or_overwriting_evidence() {
    let fixture = Fixture::new();
    fs::create_dir(&fixture.directory).unwrap();
    let head_path = fixture.directory.join("head-before.json");
    fs::write(head_path, b"original head\n").unwrap();
    let files = fixture.files();
    let fake = FakeS3::new([]);

    assert!(observe(fixture.request(), &fake, &|| Ok(())).is_err());
    fake.exhausted();
    assert_eq!(fixture.files(), files);
}

#[test]
fn rejects_unknown_metadata_and_changed_final_identity_or_owner() {
    let mut unknown = metadata();
    unknown["ObjectLockMode"] = json!("GOVERNANCE");
    let mut changed_head = metadata();
    changed_head["ETag"] = json!("\"changed-etag\"");
    let owner_length = owner_marker(SCOPE).len();
    assert_eq!(owner_marker(FOREIGN_SCOPE).len(), owner_length);
    for (steps, failure_stage, retained_stage, raw) in [
        (
            vec![Step::Owner, head(unknown.clone())],
            "head-before",
            "head-before",
            unknown,
        ),
        (
            captured_steps(metadata(), changed_head.clone(), Step::Owner),
            "capture",
            "head-after",
            changed_head,
        ),
        (
            captured_steps(metadata(), metadata(), Step::OwnerWrongScope),
            "owner-after",
            "owner-after",
            json!({"ContentLength":owner_length}),
        ),
    ] {
        let fixture = Fixture::new();
        let fake = FakeS3::new(steps);
        assert_failed(&fixture, &fake, failure_stage);
        assert_eq!(fixture.proof(&format!("{retained_stage}.json")), raw);
        let diagnostic = fixture.proof(&format!("{retained_stage}.diagnostic.json"));
        assert_eq!(diagnostic["returned"], true);
        assert_eq!(diagnostic["checkpoint_passed"], true);
        if failure_stage == "owner-after" {
            assert_eq!(
                fs::read(fixture.directory.join("owner-after-uploads.bin")).unwrap(),
                owner_marker(FOREIGN_SCOPE).as_bytes()
            );
        }
    }
}

fn captured_steps(get: Value, repeated: Value, owner: Step) -> Vec<Step> {
    vec![
        Step::Owner,
        head(metadata()),
        Step::Get { raw: Some(get) },
        Step::Head {
            conditional: true,
            raw: Some(repeated),
        },
        owner,
    ]
}

#[test]
fn checkpoint_failure_retains_raw_response_and_prevents_later_requests() {
    let fixture = Fixture::new();
    let fake = FakeS3::new([Step::Owner, head(absent())]);
    let checkpoints = Cell::new(0);
    let outcome = observe(fixture.request(), &fake, &|| {
        checkpoints.set(checkpoints.get() + 1);
        if checkpoints.get() == 4 {
            return Err("outer guard changed".into());
        }
        Ok(())
    });
    assert!(outcome.is_err());
    fake.exhausted();
    assert_eq!(fixture.proof("head-before.json"), absent());
    assert_eq!(
        fixture.proof("head-before.diagnostic.json")["checkpoint_passed"],
        false
    );
    assert_eq!(fixture.proof("failure.json")["stage"], "head-before");
    assert!(!fixture.directory.join("observation.json").exists());
}

#[test]
fn rejects_initial_evidence_drift_before_business_get() {
    let fixture = Fixture::new();
    let fake = FakeS3::new([Step::Owner, head(metadata())]);
    let changed = fixture.directory.join("head-before.json");
    let diagnostic = fixture.directory.join("head-before.diagnostic.json");
    let altered = Cell::new(false);
    let outcome = observe(fixture.request(), &fake, &|| {
        if diagnostic.is_file() && !altered.replace(true) {
            assert_eq!(fixture.proof("head-before.json"), metadata());
            alter_evidence(&changed)?;
        }
        Ok(())
    });

    assert!(outcome.is_err());
    fake.exhausted();
    assert!(altered.get());
    assert!(fs::read(changed).unwrap().starts_with(b"["));
    assert_eq!(
        fixture.proof("head-before.diagnostic.json")["returned"],
        true
    );
    let failure = fixture.proof("failure.json");
    assert_eq!(failure["stage"], "get");
    assert_eq!(failure["remote_writes"], 0);
    for name in ["get.json", "object.bin", "capture.json", "observation.json"] {
        assert!(!fixture.directory.join(name).exists());
    }
}

#[test]
fn retains_download_but_blocks_capture_of_drifted_initial_evidence() {
    let fixture = Fixture::new();
    let changed = fixture.directory.join("owner-before.diagnostic.json");
    let fake = FakeS3 {
        drift_during_get: Some(changed.clone()),
        ..FakeS3::new(captured_steps(metadata(), metadata(), Step::Owner))
    };

    assert_failed(&fixture, &fake, "binding-after");
    assert_eq!(
        fs::read(fixture.directory.join("object.bin")).unwrap(),
        b"data"
    );
    assert_eq!(fixture.proof("get.json"), metadata());
    assert_eq!(fixture.proof("get.diagnostic.json")["returned"], true);
    assert!(fs::read(changed).unwrap().starts_with(b"["));
}

#[test]
fn publication_rejects_changes_to_original_evidence_and_preserves_it() {
    for after_publication in [false, true] {
        let fixture = Fixture::new();
        let fake = FakeS3::new([Step::Owner, head(absent()), Step::Owner, head(absent())]);
        let checkpoints = Cell::new(0);
        let changed = fixture.directory.join("head-before.diagnostic.json");
        let receipt = fixture.directory.join("observation.json");
        let raw_receipt = RefCell::new(None);
        let outcome = observe(fixture.request(), &fake, &|| {
            checkpoints.set(checkpoints.get() + 1);
            if after_publication && receipt.is_file() {
                raw_receipt.replace(Some(fs::read(&receipt)?));
                fs::write(&changed, b"{}")?;
            } else if !after_publication && checkpoints.get() == 9 {
                fs::write(&changed, b"{}")?;
            }
            Ok(())
        });
        assert!(outcome.is_err());
        fake.exhausted();
        assert_eq!(fixture.proof("failure.json")["stage"], "publish");
        assert_eq!(fs::read(changed).unwrap(), b"{}");
        if after_publication {
            assert_eq!(
                fs::read(&receipt).unwrap(),
                raw_receipt.into_inner().unwrap()
            );
            assert_eq!(fixture.proof("observation.json")["status"], "object_absent");
            assert!(reject_failure(&fixture.directory).is_err());
        } else {
            assert!(!receipt.exists());
        }
    }
}

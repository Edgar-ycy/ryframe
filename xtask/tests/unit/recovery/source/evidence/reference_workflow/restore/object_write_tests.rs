use std::{cell::Cell, fs};

use super::*;

const ENDPOINT: &str = "http://127.0.0.1:9473";
const SECRET: &str = "FIXTURE_SECRET_MUST_NOT_APPEAR";

struct Fixture {
    _temp: tempfile::TempDir,
    backend: PathBuf,
    backup: PathBuf,
    work: PathBuf,
    task: Value,
    artifact: Value,
    s3: Value,
}

impl Fixture {
    fn new() -> Self {
        let managed = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&managed).unwrap();
        let temp = tempfile::tempdir_in(managed).unwrap();
        let backend = crate::recovery::fixture_control::portable_path(
            &fs::canonicalize(temp.path()).unwrap(),
        );
        let local = backend.join(".local-tests");
        let config = local.join("config");
        let work = local.join("work");
        let backup = local.join("backup");
        fs::create_dir_all(&config).unwrap();
        fs::create_dir(&work).unwrap();
        fs::create_dir_all(backup.join("objects/uploads")).unwrap();
        fs::write(
            config.join("app.test.toml"),
            format!("[object_storage]\nendpoint = \"{ENDPOINT}\"\nregion = \"us-east-1\"\naccess_key = \"fixture-access\"\nsecret_key = \"{SECRET}\"\n"),
        ).unwrap();
        let configuration =
            app_configuration::configuration_value(&backend, &config, "test").unwrap();
        let file = backup.join("objects/uploads/item.bin");
        fs::write(&file, b"bound original object").unwrap();
        let artifact = common::binding(&file).unwrap();
        let task = json!({"kind":"object","bucket":"uploads","key":"target-scope/tenant/item",
            "relative":"objects/uploads/item.bin",
            "metadata":{"CacheControl":null,"ContentDisposition":null,"ContentEncoding":null,
                "ContentLanguage":null,"ContentType":"application/octet-stream","Expires":null,
                "WebsiteRedirectLocation":null,"Metadata":{}}});
        let s3 = json!({"endpoint":ENDPOINT,"region":"us-east-1","configuration":configuration});
        Self {
            _temp: temp,
            backend,
            backup,
            work,
            task,
            artifact,
            s3,
        }
    }

    fn spec(&self) -> Create<'_> {
        Create {
            backend: &self.backend,
            work: &self.work,
            artifact_root: &self.backup,
            task: &self.task,
            artifact: &self.artifact,
            target_s3: &self.s3,
            scope: "target-scope",
        }
    }

    fn receipt(&self, diagnostic: bool) -> (PathBuf, Value) {
        let paths = fs::read_dir(&self.work)
            .unwrap()
            .map(|row| row.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .ends_with(".diagnostic.json")
                    == diagnostic
            })
            .collect::<Vec<_>>();
        let [path] = paths.as_slice() else {
            panic!("必须恰有一份原收据")
        };
        let raw = fs::read_to_string(path).unwrap();
        assert!(!raw.contains(SECRET));
        assert!(!raw.contains("fixture-access"));
        (path.clone(), document::read(path).unwrap())
    }
}

fn created() -> Value {
    json!({"status":"created_unverified","http_status":200,"remote_write_may_have_occurred":true,
        "automatic_retry_permitted":false,"response":{"ETag":"\"object\"","VersionId":"v1"}})
}

#[test]
fn persists_legacy_intent_and_pending_before_one_conditional_create() {
    let fixture = Fixture::new();
    let sends = Cell::new(0);
    let result = send_once(&fixture.spec(), &|| Ok(()), |request| {
        sends.set(sends.get() + 1);
        assert_eq!(request.bucket, "uploads");
        assert_eq!(request.key, "target-scope/tenant/item");
        assert_eq!(request.source, fixture.backup.join("objects/uploads/item.bin"));
        let (path, intent) = fixture.receipt(false);
        assert_eq!(intent, json!({"format_version":1,"kind":"ryframe-native-s3-put-if-absent-intent",
            "transport":"rust-s3-sigv4","scope_id":"target-scope","endpoint":ENDPOINT,"region":"us-east-1",
            "bucket":request.bucket,"key":request.key,"source":request.source,
            "expected":{"bytes":fixture.artifact["bytes"],"sha256":fixture.artifact["sha256"]},
            "metadata":fixture.task["metadata"]}));
        let (diagnostic, pending) = fixture.receipt(true);
        assert_eq!(diagnostic, path.with_extension("diagnostic.json"));
        assert_eq!(pending, json!({"format_version":1,"request_file":path,"outcome":"needs_reconciliation",
            "returncode":null,"error_type":"WriteResultNotRecorded","remote_write_may_have_occurred":true,
            "automatic_retry_permitted":false}));
        Ok(created())
    }).unwrap();
    assert_eq!(sends.get(), 1);
    let (path, diagnostic) = fixture.receipt(true);
    assert_eq!(result["status"], "created_unverified");
    assert_eq!(result["response"], created()["response"]);
    assert_eq!(result["diagnostic_file"], json!(path));
    assert_eq!(diagnostic["outcome"], "created_unverified");
    assert_eq!(diagnostic["returncode"], Value::Null);
}

#[test]
fn records_precondition_change_without_send_and_postcondition_change_as_unknown() {
    let fixture = Fixture::new();
    let checks = Cell::new(0);
    let checkpoint = || {
        checks.set(checks.get() + 1);
        if checks.get() == 2 {
            Err(SECRET.into())
        } else {
            Ok(())
        }
    };
    let error = send_once(&fixture.spec(), &checkpoint, |_| {
        panic!("前置失败时不得发送")
    })
    .unwrap_err();
    assert!(!error.to_string().contains(SECRET));
    let (_, diagnostic) = fixture.receipt(true);
    assert_eq!(diagnostic["outcome"], "not_started");
    assert_eq!(diagnostic["remote_write_may_have_occurred"], false);

    let fixture = Fixture::new();
    let changed = Cell::new(false);
    let checkpoint = || {
        if changed.get() {
            Err(SECRET.into())
        } else {
            Ok(())
        }
    };
    let error = send_once(&fixture.spec(), &checkpoint, |_| {
        changed.set(true);
        Ok(created())
    })
    .unwrap_err();
    assert!(!error.to_string().contains(SECRET));
    let (_, diagnostic) = fixture.receipt(true);
    assert_eq!(diagnostic["outcome"], "needs_reconciliation");
    assert_eq!(diagnostic["error_type"], "PostWriteBindingChanged");
    assert_eq!(diagnostic["remote_write_may_have_occurred"], true);
}

#[test]
fn preserves_conflict_denial_and_unknown_transport_outcomes_without_retry() {
    for (status, http_status, outcome, may_write) in [
        ("already_exists", 412, "already_exists", false),
        ("denied", 403, "denied", false),
        ("needs_reconciliation", 500, "needs_reconciliation", true),
    ] {
        let fixture = Fixture::new();
        let sends = Cell::new(0);
        assert!(
            send_once(&fixture.spec(), &|| Ok(()), |_| {
                sends.set(sends.get() + 1);
                Ok(json!({"status":status,"http_status":http_status,
                "remote_write_may_have_occurred":may_write,"automatic_retry_permitted":false}))
            })
            .is_err()
        );
        assert_eq!(sends.get(), 1);
        let (_, diagnostic) = fixture.receipt(true);
        assert_eq!(diagnostic["outcome"], outcome);
        assert_eq!(diagnostic["remote_write_may_have_occurred"], may_write);
        assert_eq!(diagnostic["automatic_retry_permitted"], false);
    }
    let fixture = Fixture::new();
    let error = send_once(&fixture.spec(), &|| Ok(()), |_| Err(SECRET.into())).unwrap_err();
    assert!(!error.to_string().contains(SECRET));
    let (_, diagnostic) = fixture.receipt(true);
    assert_eq!(diagnostic["outcome"], "needs_reconciliation");
    assert_eq!(diagnostic["error_type"], "NativePutFailed");
}

#[test]
fn rejects_changed_payload_and_unrelated_artifact_before_creating_receipts() {
    let mut fixture = Fixture::new();
    let original = fixture.backup.join("objects/uploads/item.bin");
    let other = fixture.backup.join("objects/uploads/other.bin");
    fs::copy(&original, &other).unwrap();
    let original_artifact =
        std::mem::replace(&mut fixture.artifact, common::binding(&other).unwrap());
    assert!(send_once(&fixture.spec(), &|| Ok(()), |_| panic!("禁止错绑发送")).is_err());
    assert!(fs::read_dir(&fixture.work).unwrap().next().is_none());
    fixture.artifact = original_artifact;
    fs::write(&original, b"changed original").unwrap();
    assert!(
        send_once(&fixture.spec(), &|| Ok(()), |_| panic!(
            "禁止载荷漂移后发送"
        ))
        .is_err()
    );
    assert!(fs::read_dir(&fixture.work).unwrap().next().is_none());
}

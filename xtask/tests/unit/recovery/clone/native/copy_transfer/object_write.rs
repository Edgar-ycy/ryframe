use super::*;
use std::{cell::Cell, path::PathBuf};

const ENDPOINT: &str = "http://127.0.0.1:9473";
const GENERATION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SECRET: &str = "TEST_SECRET_MUST_NOT_APPEAR";

struct Fixture {
    _temp: tempfile::TempDir,
    backend: PathBuf,
    artifacts: PathBuf,
    output: PathBuf,
    configuration: Value,
    item: Value,
}

impl Fixture {
    fn new() -> Self {
        let managed = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&managed).unwrap();
        let temp = tempfile::tempdir_in(&managed).unwrap();
        let backend = crate::recovery::fixture_control::portable_path(
            &fs::canonicalize(temp.path()).unwrap(),
        );
        let local = backend.join(".local-tests");
        let config = local.join("config");
        let artifacts = local.join("export");
        fs::create_dir_all(&config).unwrap();
        fs::create_dir(&artifacts).unwrap();
        fs::write(config.join("app.test.toml"), format!(
            "[object_storage]\nendpoint = \"{ENDPOINT}\"\nregion = \"us-east-1\"\naccess_key = \"test-access\"\nsecret_key = \"{SECRET}\"\n"
        )).unwrap();
        let configuration =
            s3::app_configuration::configuration_value(&backend, &config, "test").unwrap();
        let file = artifacts.join("payload.bin");
        fs::write(&file, b"bound original bytes").unwrap();
        let artifact = digest::file(&file).unwrap();
        let item = json!({
            "bucket":"uploads", "source_key":"source-scope/item", "target_key":"target-scope/item",
            "artifact":{"file":"payload.bin","bytes":artifact.bytes,"sha256":artifact.sha256},
            "metadata":{"CacheControl":null,"ContentDisposition":null,"ContentEncoding":null,
                "ContentLanguage":null,"ContentType":"application/octet-stream","Expires":null,
                "WebsiteRedirectLocation":null,"Metadata":{}}
        });
        let work = local.join("copy-work");
        fs::create_dir(&work).unwrap();
        let output = work.join("object-write");
        Self {
            _temp: temp,
            backend,
            artifacts,
            output,
            configuration,
            item,
        }
    }

    fn spec(&self) -> Create<'_> {
        Create {
            backend: &self.backend,
            configuration_root: &self.backend,
            artifact_root: &self.artifacts,
            work: self.output.parent().unwrap(),
            directory: &self.output,
            object: TargetObject {
                scope: "target-scope",
                endpoint: ENDPOINT,
                item: &self.item,
            },
            configuration: &self.configuration,
            generation: GENERATION,
            step: "object-first",
        }
    }

    fn artifact(&self) -> input::ArtifactBinding {
        input::bound_file(&self.artifacts, &self.item["artifact"]).unwrap()
    }

    fn diagnostic(&self) -> Value {
        document::read(&self.output.join("diagnostic.json")).unwrap()
    }
}

fn created() -> Value {
    json!({"status":"created_unverified","http_status":200,"remote_write_may_have_occurred":true,
        "automatic_retry_permitted":false,"response":{"ETag":"\"object\""}})
}

#[test]
fn records_pending_intent_before_put_and_requires_independent_observation() {
    let fixture = Fixture::new();
    let source = fixture.artifact();
    send_once(&fixture.spec(), &source, &|| Ok(()), |request| {
        assert_eq!(request.bucket, "uploads");
        assert_eq!(request.key, "target-scope/item");
        assert_eq!(request.source, fixture.artifacts.join("payload.bin"));
        assert_eq!(request.expected_sha256, source.digest.sha256);
        assert_eq!(fixture.diagnostic()["reason"], "write_result_not_recorded");
        let intent_path = fixture.output.join("intent.json");
        let intent = document::read(&intent_path).unwrap();
        assert_eq!(intent["step"], "object-first");
        assert_eq!(intent["generation_sha256"], GENERATION);
        assert!(!fs::read_to_string(intent_path).unwrap().contains(SECRET));
        Ok(created())
    })
    .unwrap();
    let diagnostic = fixture.diagnostic();
    assert_eq!(diagnostic["outcome"], "created_unverified");
    assert_eq!(diagnostic["reason"], "independent_observation_required");
    assert_eq!(
        diagnostic["response_sha256"],
        digest::plan_hash(&created()).unwrap()
    );
}

#[test]
fn changed_guard_before_send_and_during_send_have_distinct_receipts() {
    let first = Fixture::new();
    let calls = Cell::new(0);
    let check = || {
        let next = calls.get() + 1;
        calls.set(next);
        if next == 2 {
            Err("前置证据变化".into())
        } else {
            Ok(())
        }
    };
    assert!(
        send_once(&first.spec(), &first.artifact(), &check, |_| {
            panic!("前置证据变化时不能写")
        })
        .is_err()
    );
    assert_eq!(first.diagnostic()["outcome"], "not_started");
    assert_eq!(first.diagnostic()["remote_write_may_have_occurred"], false);

    let second = Fixture::new();
    let changed = Cell::new(false);
    let check = || {
        if changed.get() {
            Err("写后证据变化".into())
        } else {
            Ok(())
        }
    };
    assert!(
        send_once(&second.spec(), &second.artifact(), &check, |_| {
            changed.set(true);
            Ok(created())
        })
        .is_err()
    );
    assert_eq!(second.diagnostic()["outcome"], "needs_reconciliation");
    assert_eq!(second.diagnostic()["remote_write_may_have_occurred"], true);
    assert_eq!(second.diagnostic()["http_status"], 200);
}

#[test]
fn conflict_and_transport_failure_preserve_unknown_write_boundary_without_leaking_errors() {
    let first = Fixture::new();
    assert!(send_once(&first.spec(), &first.artifact(), &|| Ok(()), |_| {
        Ok(json!({"status":"already_exists","http_status":412,"remote_write_may_have_occurred":false}))
    }).is_err());
    assert_eq!(first.diagnostic()["outcome"], "already_exists");
    assert_eq!(first.diagnostic()["automatic_retry_permitted"], false);

    let second = Fixture::new();
    let error = send_once(&second.spec(), &second.artifact(), &|| Ok(()), |_| {
        Err(SECRET.into())
    })
    .unwrap_err();
    assert!(!error.to_string().contains(SECRET));
    assert_eq!(second.diagnostic()["outcome"], "needs_reconciliation");
    assert!(
        !fs::read_to_string(second.output.join("diagnostic.json"))
            .unwrap()
            .contains(SECRET)
    );
}

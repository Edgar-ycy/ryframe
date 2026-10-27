use super::*;
use serde_json::json;
use std::fs;

struct Fixture {
    _temporary: tempfile::TempDir,
    backend: PathBuf,
    run: PathBuf,
    environment: Value,
}

impl Fixture {
    fn new() -> Self {
        let local = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        let temporary = tempfile::tempdir_in(local).unwrap();
        let backend = operation::canonical(temporary.path()).unwrap();
        let run = backend.join("runtime 空格");
        fs::create_dir(&run).unwrap();
        for (name, id) in [("config", 1), ("worker-config", 2)] {
            let directory = run.join(name);
            fs::create_dir(&directory).unwrap();
            fs::write(directory.join("app.test.toml"), format!("scope_id=\"reserved-source-tests\"\nsnowflake_worker_id={id}\n[app]\nhost=\"127.0.0.1\"\nport=19211\n[jobs]\nmode=\"external\"\nhealth_host=\"127.0.0.1\"\nhealth_port=19212\n")).unwrap();
        }
        let executable = std::env::current_exe().unwrap();
        operation::write_new(&run.join("binaries.json"), &json!({"ryframe":executable,"ryframe-worker":executable,"ryframe-reset":executable,"ryframe-migrate":executable})).unwrap();
        let runtime = crate::recovery::fixture_control::full_stack_runtime_contract(
            &backend,
            &run,
            &run.join("config"),
            &run.join("worker-config"),
        )
        .unwrap();
        operation::write_new(&run.join("runtime.json"), &runtime).unwrap();
        let path = backend.join("environment.json");
        operation::write_new(
            &path,
            &json!({"environment":{"APP_ENV":"test","TEMP":"reserved-temp","TMP":"reserved-tmp"}}),
        )
        .unwrap();
        let mut environment = operation::file(&path).unwrap();
        environment["path"] = json!(path);
        Self {
            _temporary: temporary,
            backend,
            run,
            environment,
        }
    }
}

#[test]
fn reserved_role_id_is_readonly_until_claim_and_bound_to_persisted_request() {
    for role in [Role::Api, Role::Worker] {
        let fixture = Fixture::new();
        let reservation =
            reserve(role, &fixture.backend, &fixture.run, &fixture.environment).unwrap();
        assert!(super::super::documents::hex(&json!(reservation.id()), 32));
        assert!(!fixture.run.join(role.lock()).exists());
        let another = reserve(role, &fixture.backend, &fixture.run, &fixture.environment).unwrap();
        assert_ne!(reservation.id(), another.id());
        drop(another);
        let runtime = super::super::runtime::load(role, &fixture.backend, &fixture.run).unwrap();
        let operation =
            super::super::operation::Operation::claim_reserved(&reservation, &runtime, 90.0)
                .unwrap();
        assert_eq!(operation.id().unwrap(), reservation.id());
        assert_eq!(
            operation.owner["operation_id"],
            operation.request["operation_id"]
        );
        assert_eq!(
            operation.request["environment_binding"],
            *reservation.binding()
        );
        assert_eq!(
            operation.request["environment_binding"]["file"],
            fixture.environment
        );
        assert!(
            !serde_json::to_string(&operation.request)
                .unwrap()
                .contains("reserved-temp")
        );
        operation.check().unwrap();
    }
}

#[test]
fn consumed_reservation_rejects_runtime_drift_before_any_claim_or_spawn() {
    for role in [Role::Api, Role::Worker] {
        let fixture = Fixture::new();
        let reservation =
            reserve(role, &fixture.backend, &fixture.run, &fixture.environment).unwrap();
        let path = fixture.run.join("runtime.json");
        let mut changed = operation::read(&path).unwrap();
        changed["scope_id"] = json!("different-scope");
        crate::recovery::clone::native::json::write_replace(&path, &changed).unwrap();
        assert!(reservation.start(90.0).is_err());
        assert!(!fixture.run.join(role.lock()).exists());
        assert!(!fixture.run.join(role.archive()).exists());
        assert!(
            !fixture
                .run
                .join(format!("{}-tree.json", role.name()))
                .exists()
        );
    }
}

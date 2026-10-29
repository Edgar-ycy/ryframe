use super::super::tree;
use super::*;

fn fixture(role: Role) -> (tempfile::TempDir, Operation, Value) {
    let base = crate::workspace::root_dir().join(".local-tests");
    std::fs::create_dir_all(&base).unwrap();
    let temporary = tempfile::tempdir_in(base).unwrap();
    let directory = operation::canonical(temporary.path()).unwrap();
    let artifact = json!({"path":std::env::current_exe().unwrap(),"sha256":"d".repeat(64)});
    let runtime = json!({"scope_id":"preparation-test","api_ready_url":"http://127.0.0.1:19209/readyz",
        "worker_ready_url":"http://127.0.0.1:19210/readyz","artifacts":{"api":artifact,"worker":artifact}});
    operation::write_new(&directory.join("runtime.json"), &runtime).unwrap();
    let op = Operation::claim(role, "start", &directory, &directory, &runtime, 60.0).unwrap();
    let monitor =
        json!({"pid":2147481002_u32,"started":"1","executable":std::env::current_exe().unwrap()});
    (temporary, op, monitor)
}

#[test]
fn preparation_failure_is_bound_redacted_and_never_authorizes_a_candidate() {
    for role in [Role::Api, Role::Worker] {
        for stage in [
            Stage::RuntimeInput,
            Stage::RequestContract,
            Stage::MemberSpawn,
            Stage::MemberIdentity,
            Stage::ReadyPublication,
        ] {
            let (_directory, op, monitor) = fixture(role);
            let owner = std::fs::read(op.path.join("owner.json")).unwrap();
            let request = std::fs::read(op.path.join("request.json")).unwrap();
            let error = step::<()>(&op, &monitor, stage, || {
                Err("password=private-marker".into())
            })
            .unwrap_err();
            assert_eq!(error.to_string(), "password=private-marker");
            let path = op.path.join("preparation-failed.json");
            let bytes = std::fs::read(path).unwrap();
            assert!(!String::from_utf8_lossy(&bytes).contains("private-marker"));
            let message = failure(&op, &monitor).unwrap();
            assert!(safe_message(role, &message));
            assert!(!safe_message(
                role,
                &(message.clone() + "; password=private-marker")
            ));
            assert!(!safe_message(
                if role == Role::Api {
                    Role::Worker
                } else {
                    Role::Api
                },
                &message
            ));
            let rejected = super::super::controller::authorize(&op, &monitor).unwrap_err();
            assert_eq!(rejected.to_string(), message);
            assert!(!format!("{rejected:?}").contains("private-marker"));
            assert_eq!(std::fs::read(op.path.join("owner.json")).unwrap(), owner);
            assert_eq!(
                std::fs::read(op.path.join("request.json")).unwrap(),
                request
            );
            assert_eq!(std::fs::read_dir(&op.path).unwrap().count(), 3);
            assert!(!tree::path(op.directory(), role, op.id().unwrap(), "ready").exists());
            assert!(
                !op.directory()
                    .join(format!("{}-tree.json", role.name()))
                    .exists()
            );
        }
    }
}

#[test]
fn missing_changed_or_foreign_diagnostic_keeps_original_closed_failure() {
    let (_directory, op, monitor) = fixture(Role::Worker);
    assert!(failure(&op, &monitor).is_none());
    let original = super::super::controller::authorize(&op, &monitor)
        .unwrap_err()
        .to_string();
    #[derive(Debug)]
    struct Wrapped(std::io::Error);
    impl std::fmt::Display for Wrapped {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("private-marker")
        }
    }
    impl std::error::Error for Wrapped {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }
    let error = Wrapped(std::io::Error::from_raw_os_error(5));
    publish(&op, &monitor, Stage::MemberSpawn, &error).unwrap();
    let path = op.path.join("preparation-failed.json");
    let value = operation::read(&path).unwrap();
    assert_eq!(value["detail"]["os_code"], 5);
    validate_record(&value, op.directory(), &op.owner, &op.request).unwrap();
    assert!(
        !serde_json::to_string(&value)
            .unwrap()
            .contains("private-marker")
    );
    assert!(failure(&op, &monitor).unwrap().contains("系统码=5"));
    for (pointer, replacement) in [
        ("/role", json!("api")),
        ("/operation_id", json!("f".repeat(32))),
        ("/source_sha256", json!("f".repeat(64))),
        ("/owner_sha256", json!("f".repeat(64))),
        ("/request_sha256", json!("f".repeat(64))),
        ("/monitor/started", json!("2")),
        ("/detail/stage", json!("private-marker")),
        ("/detail/cause", json!("private-marker")),
        ("/detail/cause_sha256", json!("private-marker")),
        ("/detail/os_code", json!("private-marker")),
    ] {
        let mut changed = value.clone();
        *changed.pointer_mut(pointer).unwrap() = replacement;
        std::fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(failure(&op, &monitor).is_none());
        assert_eq!(
            super::super::controller::authorize(&op, &monitor)
                .unwrap_err()
                .to_string(),
            original
        );
    }
    let mut changed = value.clone();
    changed["detail"]["raw_error"] = json!("private-marker");
    std::fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
    assert!(failure(&op, &monitor).is_none());
    // 发布目标已有现场或不可写时，保留最初错误，不覆盖证据。
    let before = std::fs::read(&path).unwrap();
    let error = step::<()>(&op, &monitor, Stage::ReadyPublication, || {
        Err("original-failure".into())
    })
    .unwrap_err();
    assert_eq!(error.to_string(), "original-failure");
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

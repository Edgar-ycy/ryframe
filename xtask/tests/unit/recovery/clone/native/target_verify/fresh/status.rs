use super::*;
use crate::recovery::clone::native::lifecycle::state;

fn files(root: &Path) -> Value {
    tree::snapshot(&crate::workspace::root_dir(), root, false).unwrap()
}

#[test]
fn absent_incomplete_and_invalid_registration_status_never_create_a_guard_or_repair_files() {
    let backend = crate::workspace::root_dir();
    let parent = backend.join(".local-tests");
    std::fs::create_dir_all(&parent).unwrap();
    let directory = tempfile::Builder::new()
        .prefix("fresh-status-")
        .tempdir_in(parent)
        .unwrap();
    let workspace = directory.path().join("workspace");
    let before = files(directory.path());
    let absent = observe(&backend, &workspace).unwrap();
    assert_eq!(absent["next_action"], "prepare");
    assert_eq!(files(directory.path()), before);
    std::fs::create_dir(&workspace).unwrap();
    for document in [None, Some(json!({"kind":"invalid"}))] {
        if let Some(document) = document {
            std::fs::write(workspace.join("registration.json"), document.to_string()).unwrap();
        }
        let before = files(directory.path());
        let result = observe(&backend, &workspace).unwrap();
        assert_eq!(result["evidence_valid"], false);
        assert_eq!(result["next_action"], Value::Null);
        assert_eq!(files(directory.path()), before);
        assert!(!workspace.join("fresh-target.guard").exists());
    }
}

#[test]
fn fixed_window_rejects_environment_request_registration_and_complete_ledger_drift() {
    let backend = crate::workspace::root_dir();
    std::fs::create_dir_all(backend.join(".local-tests")).unwrap();
    let directory = tempfile::Builder::new()
        .prefix("fresh-window-")
        .tempdir_in(backend.join(".local-tests"))
        .unwrap();
    let workspace = directory.path().join("workspace");
    let storage = directory.path().join("storage");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(&storage).unwrap();
    std::fs::write(storage.join("manifest.json"), "{}").unwrap();
    state::initialize(&storage).unwrap();
    let request = directory.path().join("request.json");
    let environment = directory.path().join("environment.json");
    std::fs::write(&request, "{}").unwrap();
    std::fs::write(
        &environment,
        json!({"environment":{"APP_ENV":"test","TEMP":directory.path(),"TMP":directory.path()}})
            .to_string(),
    )
    .unwrap();
    let registration = json!({"format_version":1,"kind":"devex-clone-fresh-target-registration",
        "request":common::binding(&request).unwrap(),"environment":common::binding(&environment).unwrap(),
        "target_directory":workspace.join("target"),"storage_run":{"path":storage,
            "manifest":common::binding(&storage.join("manifest.json")).unwrap(),
            "state":common::binding(&storage.join("state.json")).unwrap()}});
    let path = workspace.join("registration.json");
    std::fs::write(&path, registration.to_string()).unwrap();
    for path in [&environment, &request, &path, &storage.join("state.json")] {
        let window = Window::load(&backend, &workspace).unwrap();
        let bytes = std::fs::read(path).unwrap();
        let mut altered = bytes.clone();
        altered.push(b' ');
        std::fs::write(path, altered).unwrap();
        let before = files(directory.path());
        assert!(window.check().is_err());
        assert_eq!(files(directory.path()), before);
        std::fs::write(path, bytes).unwrap();
    }
}

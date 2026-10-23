use super::*;
use serde_json::json;
use std::{fs, net::TcpListener, path::PathBuf};

struct Fixture {
    backend: PathBuf,
    directory: tempfile::TempDir,
    original: Value,
    current: Value,
}

impl Fixture {
    fn new() -> Self {
        let backend = crate::workspace::root_dir();
        let root = backend.join(".local-tests/t");
        fs::create_dir_all(&root).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("重绑定 证据-")
            .tempdir_in(root)
            .unwrap();
        let run = directory.path();
        let data = run.join("data");
        fs::create_dir(&data).unwrap();
        let (device, inode) =
            common::device_inode_for(&data, &fs::symlink_metadata(&data).unwrap()).unwrap();
        for filename in ["request.json", "old.json", "new.json"] {
            document::write_new(&run.join(filename), &json!({"fixture":filename})).unwrap();
        }
        let original = json!({"request":common::binding(&run.join("request.json")).unwrap(),
            "restart_result":common::binding(&run.join("old.json")).unwrap(),"attempt":7,
            "api_url":"http://127.0.0.1:29000","console_url":"http://127.0.0.1:29001",
            "data_directory":{"path":data,"device":device,"inode":inode},
            "storage":{"sha256":"a".repeat(64),"identity":{"pid":u32::MAX,"started":"100","executable":std::env::current_exe().unwrap()}}});
        let mut current = original.clone();
        current["attempt"] = json!(53);
        current["restart_result"] = common::binding(&run.join("new.json")).unwrap();
        current["storage"]["identity"]["started"] = json!("200");
        Self {
            backend,
            directory,
            original,
            current,
        }
    }

    fn input(&self) -> Value {
        json!({"original":self.original,"current":self.current})
    }

    fn file(&self, name: &str, value: &Value) -> Value {
        let path = self.directory.path().join(name);
        document::write_new(&path, value).unwrap();
        common::binding(&path).unwrap()
    }

    fn source(&self, api: u16, worker: u16) -> Value {
        let history = self.file("history.json", &json!({"events":["stopped"]}));
        let lineage = self.file(
            "lineage.json",
            &json!({"identities":[{"identity":self.original["storage"]["identity"]}],
            "runtimes":[{"history":history}]}),
        );
        let runtime = self.file(
            "runtime.json",
            &json!({"worker_ready_url":format!("http://127.0.0.1:{worker}")}),
        );
        json!({"registration":{"producer_lineage":lineage},"request":{"runtime":runtime,
            "source":{"api_url":format!("http://127.0.0.1:{api}")}}})
    }
}

#[test]
fn transition_preserves_all_bytes_and_directory_identity() {
    let fixture = Fixture::new();
    let request = fixture.directory.path().join("request.json");
    let before = fs::read(&request).unwrap();
    assert!(transition(&fixture.backend, &fixture.input()).is_ok());
    assert_eq!(before, fs::read(&request).unwrap());
    let mut changed = fixture.input();
    changed["original"]["data_directory"]["path"] = json!(fixture.directory.path().join("missing"));
    changed["current"]["data_directory"] = changed["original"]["data_directory"].clone();
    assert!(transition(&fixture.backend, &changed).is_err());
    fs::write(request, b"{\"credentials\":\"changed\"}").unwrap();
    assert!(transition(&fixture.backend, &fixture.input()).is_err());
}

#[test]
fn restart_files_keep_same_directory_request_and_stopped_old_identity() {
    let mut fixture = Fixture::new();
    let input = json!({"original":fixture.original});
    assert!(restart(&fixture.backend, &input).is_ok());
    fixture.original["storage"]["identity"] = common::process_identity(std::process::id())
        .unwrap()
        .unwrap();
    assert!(restart(&fixture.backend, &json!({"original":fixture.original})).is_err());
    fs::write(
        fixture.directory.path().join("request.json"),
        b"{\"other\":true}",
    )
    .unwrap();
    assert!(restart(&fixture.backend, &input).is_err());
}

#[test]
fn transition_checks_each_bound_restart_and_exact_old_creation() {
    let mut fixture = Fixture::new();
    let own = common::process_identity(std::process::id())
        .unwrap()
        .unwrap();
    fixture.original["storage"]["identity"] = own.clone();
    fixture.current["storage"]["identity"]["executable"] = own["executable"].clone();
    assert!(transition(&fixture.backend, &fixture.input()).is_err());
    fixture.original["storage"]["identity"]["pid"] = json!(u32::MAX);
    for filename in ["old.json", "new.json"] {
        let bytes = fs::read(fixture.directory.path().join(filename)).unwrap();
        fs::write(
            fixture.directory.path().join(filename),
            b"{\"changed\":true}",
        )
        .unwrap();
        assert!(transition(&fixture.backend, &fixture.input()).is_err());
        fs::write(fixture.directory.path().join(filename), bytes).unwrap();
    }
}

#[test]
fn pid_reuse_requires_strictly_newer_creation_and_inspection_errors_fail_closed() {
    let old = json!({"pid":77,"started":"100","executable":std::env::current_exe().unwrap()});
    assert!(stopped_with(&old, |_| Ok(None)).is_ok());
    for timestamp in ["1", "99", "100", "0101", "0", "18446744073709551615"] {
        let mut current = old.clone();
        current["started"] = json!(timestamp);
        assert!(
            stopped_with(&old, |_| Ok(Some(current))).is_err(),
            "{timestamp}"
        );
    }
    let mut current = old.clone();
    current["started"] = json!("101");
    assert!(stopped_with(&old, |_| Ok(Some(current.clone()))).is_ok());
    current["pid"] = json!(78);
    assert!(stopped_with(&old, |_| Ok(Some(current))).is_err());
    assert!(stopped_with(&old, |_| Err("查询失败".into())).is_err());
}

#[test]
fn quiet_checks_both_actual_ports_lineage_and_no_external_writes() {
    let api = TcpListener::bind("127.0.0.1:0").unwrap();
    let worker = TcpListener::bind("127.0.0.1:0").unwrap();
    let api_port = api.local_addr().unwrap().port();
    let worker_port = worker.local_addr().unwrap().port();
    let fixture = Fixture::new();
    let input = fixture.source(api_port, worker_port);
    assert!(quiet(&fixture.backend, &input).is_err());
    drop(api);
    assert!(quiet(&fixture.backend, &input).is_err());
    drop(worker);
    let history = fixture.directory.path().join("history.json");
    let before = fs::read(&history).unwrap();
    assert!(quiet(&fixture.backend, &input).is_ok());
    assert_eq!(before, fs::read(&history).unwrap());
    fs::write(history, b"{\"events\":[\"started\"]}").unwrap();
    assert!(quiet(&fixture.backend, &input).is_err());
}

#[test]
fn quiet_rejects_runtime_binding_and_living_producer() {
    let api = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = api.local_addr().unwrap().port();
    drop(api);
    let fixture = Fixture::new();
    let mut input = fixture.source(port, port);
    let own = common::process_identity(std::process::id())
        .unwrap()
        .unwrap();
    input["registration"]["producer_lineage"] =
        fixture.file("living.json", &json!({"identities":[{"identity":own}]}));
    assert!(quiet(&fixture.backend, &input).is_err());
    input["registration"]["producer_lineage"] =
        common::binding(&fixture.directory.path().join("lineage.json")).unwrap();
    fs::write(
        fixture.directory.path().join("runtime.json"),
        b"{\"worker_ready_url\":\"http://127.0.0.1:1\"}",
    )
    .unwrap();
    assert!(quiet(&fixture.backend, &input).is_err());
}

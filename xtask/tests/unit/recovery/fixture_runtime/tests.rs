use super::{manifest, model::*, protocol, receipts, responses};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let temp = temporary();
    let root = temp.path().join("results");
    fs::create_dir(&root).unwrap();
    (temp, root)
}
fn put(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}
fn tests_receipt() -> Value {
    json!({"format_version":1,"kind":"business-browser-tests","fixture":"business","server":"dev","run_id":"run-a","status":"passed",
        "runs":receipts::TESTS.iter().map(|(title,scenarios)|json!({"title":[title],"status":"passed","retry":0,"scenarios":scenarios})).collect::<Vec<_>>()})
}
#[test]
fn manifest_is_complete_sorted_bounded_and_rechecks_every_member() {
    let (temp, root) = fixture();
    fs::create_dir(root.join("nested")).unwrap();
    fs::write(root.join("index.html"), b"index").unwrap();
    fs::write(root.join("nested/trace.zip"), b"trace").unwrap();
    let capture = manifest::capture(&root, temp.path(), (MAX_FILES, MAX_BYTES)).unwrap();
    let v = &capture["manifest"];
    assert_eq!(v["total_bytes"], 10);
    assert_eq!(v["files"][0]["path"], "index.html");
    assert_eq!(v["files"][1]["path"], "nested/trace.zip");
    manifest::assert_unchanged(&capture).unwrap();
    fs::write(root.join("extra"), b"x").unwrap();
    assert!(manifest::verify(v, &root, temp.path()).is_err());
    assert!(manifest::capture(&root, temp.path(), (1, MAX_BYTES)).is_err());
    assert!(manifest::capture(&root, temp.path(), (MAX_FILES, 2)).is_err());
    assert!(manifest::capture(&root, temp.path(), (0, 2)).is_err());
    assert!(manifest::capture(&root, &root, (MAX_FILES, MAX_BYTES)).is_err());
}
#[test]
fn manifest_rejects_empty_traversal_and_false_totals() {
    let (temp, root) = fixture();
    assert!(manifest::capture(&root, temp.path(), (1, 1)).is_err());
    fs::write(root.join("a"), b"a").unwrap();
    let mut v =
        manifest::capture(&root, temp.path(), (MAX_FILES, MAX_BYTES)).unwrap()["manifest"].clone();
    v["files"][0]["path"] = json!("../a");
    assert!(manifest::verify(&v, &root, temp.path()).is_err());
    v["files"][0]["path"] = json!("a");
    v["total_files"] = json!(true);
    assert!(manifest::verify(&v, &root, temp.path()).is_err());
}
#[test]
fn manifest_guard_detects_a_to_b_to_a_and_replaced_identity() {
    let (temp, root) = fixture();
    let path = root.join("asset.js");
    fs::write(&path, b"A").unwrap();
    let capture = manifest::capture(&root, temp.path(), (MAX_FILES, MAX_BYTES)).unwrap();
    fs::write(&path, b"B").unwrap();
    fs::write(&path, b"A").unwrap();
    let file = fs::File::options().write(true).open(&path).unwrap();
    file.set_times(
        fs::FileTimes::new()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(1)),
    )
    .unwrap();
    assert!(manifest::assert_unchanged(&capture).is_err());
    let guard = snapshot(&path, 10).unwrap();
    let backup = root.join("backup");
    fs::rename(&path, &backup).unwrap();
    fs::write(&path, b"A").unwrap();
    assert!(unchanged(&guard).is_err());
}
#[cfg(unix)]
#[test]
fn manifest_rejects_link_escape() {
    let (temp, root) = fixture();
    let outside = temp.path().join("outside");
    fs::write(&outside, b"secret").unwrap();
    std::os::unix::fs::symlink(outside, root.join("link")).unwrap();
    assert!(manifest::capture(&root, temp.path(), (MAX_FILES, MAX_BYTES)).is_err());
}
#[test]
fn exact_four_runs_and_five_scenarios_reject_retries_duplicates_and_prefixes() {
    let (_temp, root) = fixture();
    let path = root.join("tests.json");
    let v = tests_receipt();
    put(&path, &v);
    receipts::business(&path, "dev", "run-a").unwrap();
    let mut cases = vec![];
    let mut c = v.clone();
    let extra = c["runs"][0].clone();
    c["runs"].as_array_mut().unwrap().push(extra);
    cases.push(c);
    let mut c = v.clone();
    c["runs"][0]["title"] = json!(["prefix", receipts::TESTS[0].0]);
    cases.push(c);
    let mut c = v.clone();
    c["runs"][0]["retry"] = json!(1);
    cases.push(c);
    let mut c = v.clone();
    c["runs"][0]["scenarios"] = json!(["retention"]);
    cases.push(c);
    let mut c = v.clone();
    c["runs"][0] = c["runs"][1].clone();
    cases.push(c);
    let mut c = v.clone();
    c["runs"][0]["status"] = json!("skipped");
    cases.push(c);
    let mut c = v;
    c["run_id"] = json!("other");
    cases.push(c);
    for c in cases {
        put(&path, &c);
        assert!(receipts::business(&path, "dev", "run-a").is_err());
    }
}
pub(super) fn budget_value() -> (Value, Value) {
    let binding = json!({"scope_id":"fixture-source","rate_limits":{"login":{"capacity":7,"window_secs":30}}});
    let v = json!({"version":1,"binding":{"scope":"fixture-source","capacity":7,"windowMs":30000},"observedAt":1,
        "buckets":{format!("principal:{}","a".repeat(64)):{"generation":"00000000-0000-0000-0000-000000000000","count":1,"reservedAt":0,"completedAt":1}}});
    (binding, v)
}
#[test]
fn budget_binds_scope_capacity_first_write_and_entry_bounds() {
    let (_temp, root) = fixture();
    let path = root.join("budget.json");
    let (binding, v) = budget_value();
    put(&path, &v);
    assert_eq!(receipts::budget(&path, &binding).unwrap()["ledger"], v);
    let mut cases = vec![];
    let mut c = v.clone();
    c["buckets"] = json!({});
    cases.push(c);
    let mut c = v.clone();
    c["binding"]["scope"] = json!("other");
    cases.push(c);
    let mut c = v.clone();
    c["binding"]["capacity"] = json!(1);
    cases.push(c);
    let mut c = v.clone();
    c["observedAt"] = json!(u64::MAX);
    cases.push(c);
    for field in ["count", "reservedAt", "completedAt", "generation"] {
        let mut c = v.clone();
        let entry = c["buckets"]
            .as_object_mut()
            .unwrap()
            .values_mut()
            .next()
            .unwrap();
        entry[field] = json!(-1);
        cases.push(c);
    }
    for c in cases {
        put(&path, &c);
        assert!(receipts::budget(&path, &binding).is_err());
    }
}
#[test]
fn redacted_log_rejects_secret_even_with_matching_descriptor_and_detects_drift() {
    let (_temp, root) = fixture();
    let path = root.join("browser.log");
    fs::write(&path, b"request AdminSecret").unwrap();
    let d = bound(&path).unwrap();
    assert!(read_log(&path, &d, &["AdminSecret".into()]).is_err());
    fs::write(&path, b"redacted").unwrap();
    assert!(read_log(&path, &d, &[]).is_err());
    read_log(&path, &bound(&path).unwrap(), &["AdminSecret".into()]).unwrap();
}
#[test]
fn preview_accepts_spa_and_repeated_exact_responses_without_file_bodies() {
    let (temp, root) = fixture();
    fs::write(root.join("index.html"), b"index-a").unwrap();
    fs::write(root.join("app.js"), b"script-a").unwrap();
    let manifest =
        manifest::capture(&root, temp.path(), (MAX_FILES, MAX_BYTES)).unwrap()["manifest"].clone();
    let binding = json!({"run_id":"run-a","scope_id":"scope"});
    let entries = vec![
        response_entry(1, "/system/business", &root.join("index.html"), "document"),
        response_entry(2, "/app.js", &root.join("app.js"), "script"),
        response_entry(3, "/app.js", &root.join("app.js"), "script"),
    ];
    let path = temp.path().join("responses.json");
    let v = response_receipt(&entries);
    put(&path, &v);
    let result = responses::preview(&path, &binding, &manifest).unwrap();
    assert_eq!(result["receipt"], v);
    assert!(!result.to_string().contains("script-a"));
    for field in [
        "method",
        "destination",
        "status",
        "sha256",
        "bytes",
        "representation",
        "sequence",
        "path",
    ] {
        let mut changed = v.clone();
        changed["entries"][0][field] = json!("invalid");
        put(&path, &changed);
        assert!(
            responses::preview(&path, &binding, &manifest).is_err(),
            "{field}"
        );
    }
}
fn response_entry(i: u64, path: &str, file: &Path, destination: &str) -> Value {
    let b = bound(file).unwrap();
    json!({"sequence":i,"method":"GET","path":path,"destination":destination,"status":200,"bytes":b["bytes"],"sha256":b["sha256"],"representation":"identity"})
}
fn response_receipt(entries: &[Value]) -> Value {
    json!({"format_version":1,"kind":"business-preview-static-responses","status":"complete","run_id":"run-a","scope_id":"scope","limits":{"entries":MAX_FILES,"bytes":MAX_BYTES},"total_entries":entries.len(),"total_bytes":entries.iter().map(|e|e["bytes"].as_u64().unwrap()).sum::<u64>(),"entries":entries})
}
#[test]
fn response_paths_reject_query_traversal_noncanonical_escape_and_unknown_targets() {
    for path in [
        "/app.js?v=1",
        "/app.js#x",
        "//app.js",
        "/../app.js",
        "/%61pp.js",
        "/%ff",
        "/a\\b",
        "relative",
        "/a/./b",
        "/a//b",
    ] {
        assert!(responses::target(path).is_err(), "{path}");
    }
    assert_eq!(responses::target("/login").unwrap(), "index.html");
    assert_eq!(
        responses::target("/assets/%E4%B8%AD.js").unwrap(),
        "assets/中.js"
    );
    let (temp, root) = fixture();
    fs::write(root.join("index.html"), b"a").unwrap();
    fs::write(root.join("app.js"), b"b").unwrap();
    let manifest =
        manifest::capture(&root, temp.path(), (MAX_FILES, MAX_BYTES)).unwrap()["manifest"].clone();
    let binding = json!({"run_id":"run-a","scope_id":"scope"});
    let path = temp.path().join("audit.json");
    let entries = [
        response_entry(1, "/missing.js", &root.join("app.js"), "script"),
        response_entry(2, "/", &root.join("index.html"), "document"),
    ];
    put(&path, &response_receipt(&entries));
    assert!(responses::preview(&path, &binding, &manifest).is_err());
}
#[test]
fn private_protocol_rejects_duplicate_fields_wrong_write_and_unknown_operation() {
    for raw in [
        r#"{"operation":"manifest","operation":"terminal","arguments":{},"write":false}"#,
        r#"{"operation":"terminal","arguments":{},"write":false}"#,
        r#"{"operation":"other","arguments":{},"write":false}"#,
    ] {
        assert!(protocol::execute(raw.as_bytes()).is_err());
    }
}

pub(super) fn temporary() -> tempfile::TempDir {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".local-tests");
    std::fs::create_dir_all(&root).unwrap();
    tempfile::Builder::new()
        .prefix("fixture-browser-")
        .tempdir_in(root)
        .unwrap()
}

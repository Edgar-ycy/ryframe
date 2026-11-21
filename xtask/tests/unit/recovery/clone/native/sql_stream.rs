use std::{
    fs,
    io::{BufReader, Cursor, Read},
    path::PathBuf,
};

use super::{
    super::{digest, sql},
    request,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

struct Fixture {
    _directory: tempfile::TempDir,
    backend: PathBuf,
    source: PathBuf,
    catalog: Value,
}

impl Fixture {
    fn new(body: &str) -> Self {
        let parent = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&parent).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("sql-stream-test-")
            .tempdir_in(parent)
            .unwrap();
        let backend = directory.path().join("backend");
        fs::create_dir_all(backend.join(".local-tests")).unwrap();
        let source = directory.path().join("数据 source.sql");
        fs::write(&source, format!("{}\n{body}", sql::DATA_HEADER)).unwrap();
        Self {
            _directory: directory,
            backend,
            source,
            catalog: json!({
            "a": {"id":"BIGINT", "payload":"JSON", "text":"TEXT", "raw":"BLOB"},
            "b": {"id":"BIGINT"}, "empty": {"id":"BIGINT"}}),
        }
    }

    fn args(&self, selected: Option<&str>) -> Value {
        let expected = digest::file(&self.source).unwrap();
        json!({"backend":self.backend,"path":self.source,
            "expected":{"bytes":expected.bytes,"sha256":expected.sha256},
            "catalog":self.catalog,"only_table":selected})
    }

    fn create(&self, selected: Option<&str>) -> Value {
        request("reference-sql-stream-create", &self.args(selected)).unwrap()
    }
}

fn body() -> String {
    "INSERT INTO `a` (`id`, `payload`, `text`, `raw`) VALUES (123456789012345678901, '{\"n\":123456789012345678901,\"d\":0.123456789012345678901}', 'quote\\\' slash\\\\', 0x00ff);\nINSERT INTO `b` (`id`) VALUES (NULL);\n".into()
}

fn collect(stream: &Value) -> (String, usize) {
    let mut offset = 0_u64;
    let mut text = String::new();
    let mut chunks = 0;
    loop {
        let part = request(
            "reference-sql-stream-read",
            &json!({"stream":stream,"offset":offset}),
        )
        .unwrap();
        let chunk = part["text"].as_str().unwrap();
        assert!(chunk.len() <= 4 * 1024 * 1024);
        assert!(serde_json::to_vec(&part).unwrap().len() < 16 * 1024 * 1024);
        assert_eq!(part["offset"], offset);
        assert_eq!(part["next_offset"], offset + chunk.len() as u64);
        assert_eq!(
            part["chunk_sha256"],
            hex::encode(Sha256::digest(chunk.as_bytes()))
        );
        offset = part["next_offset"].as_u64().unwrap();
        text.push_str(chunk);
        chunks += 1;
        assert_eq!(
            part["eof"],
            offset == stream["artifact"]["bytes"].as_u64().unwrap()
        );
        if part["eof"] == true {
            break;
        }
        assert!(!chunk.is_empty());
    }
    assert_eq!(offset, stream["artifact"]["bytes"]);
    assert_eq!(
        hex::encode(Sha256::digest(text.as_bytes())),
        stream["artifact"]["sha256"]
    );
    (text, chunks)
}

#[test]
fn exact_typed_rows_all_table_counts_and_empty_selection_roundtrip() {
    let fixture = Fixture::new(&body());
    let original = fs::read(&fixture.source).unwrap();
    for selected in [None, Some("a"), Some("empty")] {
        let stream = fixture.create(selected);
        assert_eq!(stream["tables"], json!({"a":1,"b":1,"empty":0}));
        assert_eq!(stream["source_rows"], 2);
        let (text, _) = collect(&stream);
        let rows: Vec<Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(rows.len() as u64, stream["rows"]);
        if selected != Some("empty") {
            let row = &rows[0]["row"];
            assert_eq!(
                row["id"],
                json!({"kind":"number","value":"123456789012345678901"})
            );
            assert_eq!(row["raw"], json!({"kind":"bytes","value":"00ff"}));
            assert_eq!(
                row["text"],
                json!({"kind":"text","value":r#"quote' slash\"#})
            );
            assert_eq!(row["payload"]["kind"], "json");
            assert_eq!(
                row["payload"]["value"]["n"].to_string(),
                "123456789012345678901"
            );
            assert_eq!(
                row["payload"]["value"]["d"].to_string(),
                "0.123456789012345678901"
            );
        }
        if selected.is_none() {
            assert_eq!(rows[1]["row"]["id"], json!({"kind":"null"}));
        }
        let root = PathBuf::from(stream["root"].as_str().unwrap());
        assert_eq!(
            request("reference-sql-stream-close", &json!({"stream":stream})).unwrap(),
            true
        );
        assert!(!root.exists());
        assert_eq!(fs::read(&fixture.source).unwrap(), original);
    }
    let empty = Fixture::new("");
    let stream = empty.create(None);
    assert_eq!(stream["tables"], json!({"a":0,"b":0,"empty":0}));
    assert!(collect(&stream).0.is_empty());
    request("reference-sql-stream-close", &json!({"stream":stream})).unwrap();
}

#[test]
fn malformed_unselected_tail_never_returns_ready_or_changes_source() {
    for tail in [
        "DROP TABLE a;",
        "INSERT INTO `unknown` (`id`) VALUES (1);",
        "INSERT INTO `a` (`id`) VALUES (1);",
        "INSERT INTO `a` (`id`, `id`, `text`, `raw`) VALUES (1, 2, 'x', NULL);",
        "INSERT INTO `b` (`id`) VALUES (SLEEP(1));",
        "INSERT INTO `b` (`id`) VALUES (1); DELETE FROM b;",
        "INSERT INTO `a` (`id`, `payload`, `text`, `raw`) VALUES (1, '{\"x\":1,\"x\":2}', 'x', NULL);",
        "INSERT INTO `a` (`id`, `payload`, `text`, `raw`) VALUES (1, '{\"x\":NaN}', 'x', NULL);",
        "INSERT INTO `a` (`id`, `payload`, `text`, `raw`) VALUES (1, '{}', 'bad\\q', NULL);",
        "",
        "-- ignored tail",
    ] {
        let fixture = Fixture::new(&format!("INSERT INTO `b` (`id`) VALUES (7);\n{tail}\n"));
        let before = fs::read(&fixture.source).unwrap();
        assert!(request("reference-sql-stream-create", &fixture.args(Some("b"))).is_err());
        assert_eq!(fs::read(&fixture.source).unwrap(), before);
        let roots: Vec<_> = fs::read_dir(fixture.backend.join(".local-tests/sql-row-streams"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(roots.len(), 1);
        let manifest: Value =
            serde_json::from_slice(&fs::read(roots[0].join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest["state"], "preparing");
    }
    let catalog = serde_json::from_value(Fixture::new("").catalog).unwrap();
    let endless =
        Cursor::new(format!("{}\n", sql::DATA_HEADER).into_bytes()).chain(std::io::repeat(b'x'));
    assert!(
        sql::for_each_reader(BufReader::new(endless), &catalog, None, |_, _| panic!(
            "超限行不能发布"
        ))
        .is_err()
    );
}

#[test]
fn binding_drift_and_invalid_descriptors_fail_before_any_cleanup() {
    for member in [
        "source",
        "rows.jsonl",
        "manifest.json",
        "descriptor",
        "root",
        "identity",
        "extra",
    ] {
        let fixture = Fixture::new(&body());
        let mut stream = fixture.create(None);
        let root = PathBuf::from(stream["root"].as_str().unwrap());
        match member {
            "source" => {
                fs::write(&fixture.source, "changed").unwrap();
            }
            "descriptor" => stream["source_rows"] = json!(99),
            "root" => stream["root"] = json!(fixture.backend),
            "identity" => {
                stream["artifact"]["inode"] =
                    json!(stream["artifact"]["inode"].as_u64().unwrap() ^ 1)
            }
            "extra" => {
                fs::write(root.join("unknown"), "keep").unwrap();
            }
            name => {
                fs::write(root.join(name), "changed").unwrap();
            }
        }
        assert!(
            request(
                "reference-sql-stream-read",
                &json!({"stream":stream,"offset":0})
            )
            .is_err()
        );
        let source = fs::read(&fixture.source).unwrap();
        let rows = fs::read(root.join("rows.jsonl")).unwrap();
        let manifest = fs::read(root.join("manifest.json")).unwrap();
        let error = request("reference-sql-stream-close", &json!({"stream":stream}))
            .unwrap_err()
            .to_string();
        assert!(error.contains("均未删除"));
        assert_eq!(fs::read(&fixture.source).unwrap(), source);
        assert_eq!(fs::read(root.join("rows.jsonl")).unwrap(), rows);
        assert_eq!(fs::read(root.join("manifest.json")).unwrap(), manifest);
    }
    let fixture = Fixture::new(&body());
    let mut args = fixture.args(None);
    args["expected"]["sha256"] = json!("0".repeat(64));
    assert!(request("reference-sql-stream-create", &args).is_err());
    assert!(
        !fixture
            .backend
            .join(".local-tests/sql-row-streams")
            .exists()
    );
}

#[test]
fn utf8_chunks_are_bounded_and_cursor_rejection_preserves_result() {
    let text = "中\"\\".repeat(700_000);
    let escaped = text.replace('\\', "\\\\").replace('\'', "\\'");
    let fixture = Fixture::new(&format!(
        "INSERT INTO `a` (`id`, `payload`, `text`, `raw`) VALUES (1, '{{}}', '{escaped}', NULL);\n"
    ));
    let stream = fixture.create(None);
    let (wire, chunks) = collect(&stream);
    assert!(chunks > 1);
    let row: Value = serde_json::from_str(wire.trim_end()).unwrap();
    assert_eq!(row["row"]["text"]["value"], text);
    assert!(
        request(
            "reference-sql-stream-read",
            &json!({"stream":stream,"offset":u64::MAX})
        )
        .is_err()
    );
    assert!(
        PathBuf::from(stream["root"].as_str().unwrap())
            .join("rows.jsonl")
            .exists()
    );
    request("reference-sql-stream-close", &json!({"stream":stream})).unwrap();
}

#[cfg(windows)]
#[test]
fn close_reports_partial_local_deletion_without_retry() {
    use std::{fs::OpenOptions, os::windows::fs::OpenOptionsExt};

    const FILE_SHARE_READ: u32 = 0x0000_0001;
    let fixture = Fixture::new(&body());
    let stream = fixture.create(None);
    let root = PathBuf::from(stream["root"].as_str().unwrap());
    let manifest = root.join("manifest.json");
    let before = fs::read(&fixture.source).unwrap();
    // 允许关闭前复核原文件，但持有期间拒绝删除。
    let manifest_handle = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&manifest)
        .unwrap();
    let error = request("reference-sql-stream-close", &json!({"stream":stream}))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("已删除 1 个固定文件")
            && error.contains("manifest.json")
            && error.contains("不自动重试")
    );
    assert!(!root.join("rows.jsonl").exists());
    assert!(manifest.exists() && root.exists());
    assert_eq!(fs::read(&fixture.source).unwrap(), before);
    drop(manifest_handle);
}

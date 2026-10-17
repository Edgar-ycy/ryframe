use std::io::Write;

use serde_json::{Value, json};

use super::release::{
    archive::{
        ARCHIVE_RECEIPT_LIMITS, BUILD_EVIDENCE, FIXTURE_RECEIPT, RUNTIME_EVIDENCE, RUNTIME_RECEIPT,
        SOURCE_PAIR_RECEIPT, read_archive, validate_archive_evidence,
    },
    evidence::EMPTY_SHA256,
    support::{parse_json, sha256},
};

struct ArchiveFixture {
    backend_sha: String,
    frontend_sha: String,
    run_id: u64,
    attempt: u64,
    receipt_root: String,
    fixture_hash: String,
}

impl ArchiveFixture {
    fn new() -> Self {
        Self {
            backend_sha: "a".repeat(40),
            frontend_sha: "b".repeat(40),
            run_id: 123,
            attempt: 2,
            receipt_root: "/home/runner/work/_temp/ryframe-full-stack".to_owned(),
            fixture_hash: sha256(b"business fixture"),
        }
    }

    fn snapshot(&self, head: &str, clean: bool, files: Value) -> Value {
        json!({
            "head": head,
            "patch_sha256": if clean {
                EMPTY_SHA256.to_owned()
            } else {
                sha256(format!("generated-{head}").as_bytes())
            },
            "files": files,
        })
    }

    fn clean_sources(&self) -> Value {
        json!({
            "backend": self.snapshot(&self.backend_sha, true, json!([])),
            "frontend": self.snapshot(&self.frontend_sha, true, json!([])),
        })
    }

    fn pair(&self, sources: &Value) -> (Value, Vec<u8>, Value) {
        let pair = json!({
            "format_version": 1,
            "backend_sha": self.backend_sha,
            "frontend_sha": self.frontend_sha,
            "run_id": self.run_id,
            "attempt": self.attempt,
            "sources": sources,
        });
        let pair_raw = raw(&pair);
        let mut embedded = pair.clone();
        embedded["receipt"] = self.binding(
            &format!("{}/{SOURCE_PAIR_RECEIPT}", self.receipt_root),
            &pair_raw,
        );
        (pair, pair_raw, embedded)
    }

    fn source(
        &self,
        fixture: &str,
        sources: &Value,
        embedded: Value,
    ) -> (String, Value, Option<(Vec<u8>, Value)>) {
        if fixture == "core" {
            (
                "/home/runner/work/ryframe/ryframe/backend".to_owned(),
                json!({
                    "format_version": 1,
                    "fixture": "core",
                    "backend_root": "/home/runner/work/ryframe/ryframe/backend",
                    "source_pair": embedded,
                    "original": {"backend": sources["backend"]},
                }),
                None,
            )
        } else {
            self.business_source(sources, embedded)
        }
    }

    fn business_source(
        &self,
        sources: &Value,
        embedded: Value,
    ) -> (String, Value, Option<(Vec<u8>, Value)>) {
        let fixture_root =
            "/home/runner/work/ryframe/ryframe/backend/.local-tests/business-fixture";
        let backend_root = format!("{fixture_root}/backend");
        let roots = json!({
            "backend": backend_root,
            "frontend": format!("{fixture_root}/frontend"),
        });
        let generated = json!({
            "backend": self.snapshot(
                &self.backend_sha,
                false,
                json!([{
                    "path": "crates/order-business/src/resources/mod.rs",
                    "sha256": self.fixture_hash,
                }]),
            ),
            "frontend": sources["frontend"],
        });
        let receipt = json!({
            "format_version": 1,
            "fixture": "business",
            "status": "ready",
            "fixture_sha256": self.fixture_hash,
            "sources": sources,
            "paths": roots,
            "generated": generated,
        });
        let receipt_raw = raw(&receipt);
        let source = json!({
            "format_version": 1,
            "fixture": "business",
            "fixture_receipt": self.binding(
                &format!("{fixture_root}/{FIXTURE_RECEIPT}"),
                &receipt_raw,
            ),
            "fixture_definition": {
                "model": {
                    "path": format!(
                        "{backend_root}/tools/python/fixtures/order-business/src/resources/mod.rs"
                    ),
                    "bytes": b"business fixture".len(),
                    "sha256": self.fixture_hash,
                },
            },
            "roots": roots,
            "source_pair": embedded,
            "original": sources,
            "generated": generated,
        });
        (backend_root, source, Some((receipt_raw, receipt)))
    }

    fn artifacts(&self) -> serde_json::Map<String, Value> {
        [
            "ryframe",
            "ryframe-worker",
            "ryframe-reset",
            "ryframe-migrate",
        ]
        .into_iter()
        .map(|role| {
            (
                role.to_owned(),
                json!({
                    "path": format!("/home/runner/work/target/ci/full-stack/{role}"),
                    "bytes": role.len(),
                    "sha256": sha256(role.as_bytes()),
                }),
            )
        })
        .collect()
    }

    fn archive(&self, fixture: &str) -> (Vec<(String, Vec<u8>)>, Value) {
        let sources = self.clean_sources();
        let (pair, pair_raw, embedded_pair) = self.pair(&sources);
        let (backend_root, source, fixture_receipt) = self.source(fixture, &sources, embedded_pair);
        let artifacts = self.artifacts();
        let build = json!({
            "format_version": 1,
            "kind": "full-stack-build",
            "backend_root": backend_root,
            "source": source,
            "artifacts": artifacts,
        });
        let build_raw = raw(&build);
        let runtime = json!({
            "format_version": 1,
            "backend_root": backend_root,
            "scope_id": format!("ci-{}-{}", self.run_id, self.attempt),
            "configuration_sha256": "d".repeat(64),
            "worker_ready_url": "http://127.0.0.1:9091/readyz",
            "artifacts": {
                "api": {
                    "path": artifacts["ryframe"]["path"],
                    "sha256": artifacts["ryframe"]["sha256"],
                },
                "worker": {
                    "path": artifacts["ryframe-worker"]["path"],
                    "sha256": artifacts["ryframe-worker"]["sha256"],
                },
            },
        });
        let runtime_raw = raw(&runtime);
        let runtime_evidence = json!({
            "format_version": 1,
            "kind": "full-stack-runtime",
            "source": source,
            "build_evidence": self.binding(
                &format!("{}/{BUILD_EVIDENCE}", self.receipt_root),
                &build_raw,
            ),
            "runtime": self.binding(
                &format!("{}/{RUNTIME_RECEIPT}", self.receipt_root),
                &runtime_raw,
            ),
        });
        let mut members = vec![
            ("artifact/logs/source-pair.json".to_owned(), pair_raw),
            ("artifact/logs/build-evidence.json".to_owned(), build_raw),
            ("artifact/logs/runtime.json".to_owned(), runtime_raw),
            (
                "artifact/logs/runtime-evidence.json".to_owned(),
                raw(&runtime_evidence),
            ),
        ];
        if let Some((fixture_raw, _)) = &fixture_receipt {
            members.push(("artifact/logs/fixture.json".to_owned(), fixture_raw.clone()));
        }
        let values = json!({
            "pair": pair,
            "build": build,
            "runtime": runtime,
            "runtime_evidence": runtime_evidence,
            "fixture": fixture_receipt.map(|(_, value)| value),
        });
        (members, values)
    }

    fn binding(&self, path: &str, raw: &[u8]) -> Value {
        json!({"path": path, "bytes": raw.len(), "sha256": sha256(raw)})
    }

    fn validate(&self, entries: &[(String, Vec<u8>)], fixture: &str) -> crate::Result<Value> {
        validate_archive_evidence(
            entries,
            fixture,
            &self.backend_sha,
            &self.frontend_sha,
            self.run_id,
            self.attempt,
            (fixture == "business").then_some(self.fixture_hash.as_str()),
        )
    }
}

fn raw(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

fn replace(
    entries: &[(String, Vec<u8>)],
    name: &str,
    replacement: Vec<u8>,
) -> Vec<(String, Vec<u8>)> {
    entries
        .iter()
        .map(|(path, raw)| {
            (
                path.clone(),
                if path.rsplit('/').next() == Some(name) {
                    replacement.clone()
                } else {
                    raw.clone()
                },
            )
        })
        .collect()
}

fn member<'a>(entries: &'a [(String, Vec<u8>)], name: &str) -> &'a [u8] {
    entries
        .iter()
        .find(|(path, _)| path.rsplit('/').next() == Some(name))
        .map(|(_, raw)| raw.as_slice())
        .unwrap()
}

#[test]
fn core_archive_binds_clean_source_roles_and_runtime_receipts() {
    let fixture = ArchiveFixture::new();
    let (entries, values) = fixture.archive("core");
    let result = fixture.validate(&entries, "core").unwrap();
    assert_eq!(result["source_pair"], values["pair"]);
    assert_eq!(result["build_evidence"], values["build"]);
    assert_eq!(result["runtime"], values["runtime"]);
    assert_eq!(result["runtime_evidence"], values["runtime_evidence"]);
}

#[test]
fn business_archive_binds_fixture_raw_bytes_and_generated_model() {
    let fixture = ArchiveFixture::new();
    let (entries, values) = fixture.archive("business");
    let result = fixture.validate(&entries, "business").unwrap();
    assert_eq!(result["fixture_receipt"], values["fixture"]);
    assert_eq!(
        result["build_evidence"]["source"]["original"],
        values["fixture"]["sources"]
    );
    assert!(
        validate_archive_evidence(
            &entries,
            "business",
            &fixture.backend_sha,
            &fixture.frontend_sha,
            fixture.run_id,
            fixture.attempt,
            Some(&"e".repeat(64)),
        )
        .is_err()
    );
}

#[test]
fn every_receipt_rejects_duplicate_json_fields() {
    let fixture = ArchiveFixture::new();
    let (entries, _) = fixture.archive("business");
    for (name, _) in ARCHIVE_RECEIPT_LIMITS {
        let invalid = replace(&entries, name, b"{\"field\":1,\"field\":2}".to_vec());
        let error = fixture.validate(&invalid, "business").unwrap_err();
        assert!(error.to_string().contains("重复字段"), "{name}: {error}");
    }
}

#[test]
fn missing_duplicate_and_noncanonical_members_fail_closed() {
    let fixture = ArchiveFixture::new();
    let (entries, _) = fixture.archive("core");
    assert!(
        fixture
            .validate(&entries[..entries.len() - 1], "core")
            .is_err()
    );
    let mut duplicate = entries.clone();
    duplicate.push((
        "copy/runtime.json".to_owned(),
        member(&entries, RUNTIME_RECEIPT).to_vec(),
    ));
    assert!(fixture.validate(&duplicate, "core").is_err());
    let mut escaped = vec![("../escape.txt".to_owned(), b"ignored".to_vec())];
    escaped.extend_from_slice(&entries);
    assert!(fixture.validate(&escaped, "core").is_err());
}

#[test]
fn unknown_schema_missing_role_and_dirty_source_fail_closed() {
    let fixture = ArchiveFixture::new();
    let (entries, values) = fixture.archive("core");
    let mut unknown = values["runtime_evidence"].clone();
    unknown["unknown"] = json!(true);
    assert!(
        fixture
            .validate(&replace(&entries, RUNTIME_EVIDENCE, raw(&unknown)), "core")
            .is_err()
    );

    let mut missing_role = values["build"].clone();
    missing_role["artifacts"]
        .as_object_mut()
        .unwrap()
        .remove("ryframe-reset");
    assert!(
        fixture
            .validate(
                &replace(&entries, BUILD_EVIDENCE, raw(&missing_role)),
                "core"
            )
            .is_err()
    );

    let mut dirty = values["pair"].clone();
    dirty["sources"]["backend"]["patch_sha256"] = json!("c".repeat(64));
    assert!(
        fixture
            .validate(&replace(&entries, SOURCE_PAIR_RECEIPT, raw(&dirty)), "core")
            .is_err()
    );
}

#[test]
fn runtime_paths_and_receipt_bindings_must_share_one_directory() {
    let fixture = ArchiveFixture::new();
    let (entries, values) = fixture.archive("core");
    let mut runtime = values["runtime"].clone();
    runtime["artifacts"]["api"]["path"] = json!("/other/ryframe");
    let runtime_raw = raw(&runtime);
    let mut runtime_evidence = values["runtime_evidence"].clone();
    runtime_evidence["runtime"] = fixture.binding(
        &format!("{}/{RUNTIME_RECEIPT}", fixture.receipt_root),
        &runtime_raw,
    );
    let changed = replace(
        &replace(&entries, RUNTIME_RECEIPT, runtime_raw),
        RUNTIME_EVIDENCE,
        raw(&runtime_evidence),
    );
    assert!(fixture.validate(&changed, "core").is_err());

    let mut split = values["runtime_evidence"].clone();
    split["build_evidence"]["path"] = json!("/other/build-evidence.json");
    split["runtime"]["path"] = json!("/other/runtime.json");
    assert!(
        fixture
            .validate(&replace(&entries, RUNTIME_EVIDENCE, raw(&split)), "core")
            .is_err()
    );
}

#[test]
fn archive_reader_rejects_oversize_receipt_before_validation() {
    let root = std::env::temp_dir().join(format!("ryframe-release-archive-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("oversize.zip");
    let file = std::fs::File::create(&path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    archive.start_file(SOURCE_PAIR_RECEIPT, options).unwrap();
    archive.write_all(&vec![b'x'; 16 * 1024 + 1]).unwrap();
    archive.finish().unwrap();
    let error = read_archive(&path).unwrap_err();
    assert!(error.to_string().contains("大小"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn json_parser_rejects_nested_duplicate_fields_with_limit() {
    let error = parse_json(br#"{"outer":{"field":1,"field":2}}"#, "嵌套证据", 1024).unwrap_err();
    assert!(error.to_string().contains("重复字段"));
    assert!(parse_json(b"{}", "空限制", 1).is_err());
}

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use serde_json::{Map, Value, json};

use super::contract::{sha256_hex, verify_contract_source};

const REPOSITORY: &str = "Edgar-ycy/ryframe";
const OPENAPI: &[u8] = b"{\"openapi\":\"3.1.0\",\"info\":{\"title\":\"RyFrame API\"}}\n";
static NEXT_DIR: AtomicU64 = AtomicU64::new(1);

struct SourceFixture {
    root: PathBuf,
    backend: PathBuf,
    frontend: PathBuf,
    candidate: PathBuf,
    source: String,
    head: String,
}

impl SourceFixture {
    fn new() -> Self {
        Self::with_openapi(OPENAPI)
    }

    fn with_openapi(openapi: &[u8]) -> Self {
        let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "ryframe-contract-source-{}-{id}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let backend = root.join("backend");
        let frontend = root.join("frontend");
        fs::create_dir_all(backend.join("openapi")).unwrap();
        fs::create_dir_all(frontend.join("openapi")).unwrap();
        run_git(&backend, &["init", "--quiet", "--initial-branch=main"]);
        run_git(&backend, &["config", "core.autocrlf", "false"]);
        fs::write(backend.join("openapi/openapi.json"), openapi).unwrap();
        run_git(&backend, &["add", "openapi/openapi.json"]);
        commit(&backend, "contract");
        let source = git_text(&backend, &["rev-parse", "HEAD"]);
        fs::write(backend.join("README.md"), "head\n").unwrap();
        run_git(&backend, &["add", "README.md"]);
        commit(&backend, "head");
        let head = git_text(&backend, &["rev-parse", "HEAD"]);
        let candidate = frontend.join("candidate.json");
        fs::write(frontend.join("openapi/openapi.json"), openapi).unwrap();
        fs::write(&candidate, openapi).unwrap();
        let fixture = Self {
            root,
            backend,
            frontend,
            candidate,
            source,
            head,
        };
        fixture.write_metadata(Map::new());
        fixture
    }

    fn metadata(&self) -> Map<String, Value> {
        let openapi = fs::read(self.frontend.join("openapi/openapi.json")).unwrap();
        let Value::Object(metadata) = json!({
            "schema_version": 1,
            "backend_repository": REPOSITORY,
            "backend_commit": self.source,
            "openapi_path": "openapi/openapi.json",
            "openapi_version": "3.1.0",
            "sha256": sha256_hex(&openapi),
        }) else {
            unreachable!()
        };
        metadata
    }

    fn write_metadata(&self, overrides: Map<String, Value>) {
        let mut metadata = self.metadata();
        metadata.extend(overrides);
        fs::write(
            self.frontend.join("openapi/source.json"),
            serde_json::to_vec_pretty(&metadata).unwrap(),
        )
        .unwrap();
    }

    fn write_raw_metadata(&self, document: impl AsRef<[u8]>) {
        fs::write(self.frontend.join("openapi/source.json"), document.as_ref()).unwrap();
    }

    fn verify(&self) -> crate::Result<String> {
        verify_contract_source(
            &self.backend,
            &self.head,
            REPOSITORY,
            &self.frontend.join("openapi/source.json"),
            &self.frontend.join("openapi/openapi.json"),
            &self.candidate,
        )
    }

    fn verify_with(&self, head: &str, repository: &str) -> crate::Result<String> {
        verify_contract_source(
            &self.backend,
            head,
            repository,
            &self.frontend.join("openapi/source.json"),
            &self.frontend.join("openapi/openapi.json"),
            &self.candidate,
        )
    }
}

impl Drop for SourceFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn run_git(repository: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(repository)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        arguments.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_text(repository: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(repository)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn commit(repository: &Path, message: &str) {
    run_git(
        repository,
        &[
            "-c",
            "user.name=RyFrame CI",
            "-c",
            "user.email=ci@example.invalid",
            "commit",
            "--quiet",
            "-m",
            message,
        ],
    );
}

fn one_override(key: &str, value: Value) -> Map<String, Value> {
    [(key.to_owned(), value)].into_iter().collect()
}

fn error(result: crate::Result<String>) -> String {
    result.expect_err("校验应失败").to_string()
}

#[test]
fn accepts_head_or_ancestor_with_identical_openapi() {
    let fixture = SourceFixture::new();
    assert_eq!(fixture.verify().unwrap(), fixture.source);
    fixture.write_metadata(one_override("backend_commit", json!(fixture.head)));
    assert_eq!(fixture.verify().unwrap(), fixture.head);
}

#[test]
fn rejects_source_frontend_or_candidate_byte_drift() {
    let fixture = SourceFixture::new();
    let drift = b"{\"openapi\":\"3.1.0\",\"info\":{\"title\":\"Drift\"}}\n";
    fs::write(&fixture.candidate, drift).unwrap();
    assert!(error(fixture.verify()).contains("后端候选 OpenAPI"));

    fs::write(fixture.frontend.join("openapi/openapi.json"), drift).unwrap();
    fixture.write_metadata(Map::new());
    assert!(error(fixture.verify()).contains("前端正式 OpenAPI"));
}

#[test]
fn rejects_non_ancestor_missing_source_and_wrong_checkout_head() {
    let fixture = SourceFixture::new();
    let tree = git_text(
        &fixture.backend,
        &["rev-parse", &format!("{}^{{tree}}", fixture.source)],
    );
    let side = git_text(
        &fixture.backend,
        &[
            "-c",
            "user.name=RyFrame CI",
            "-c",
            "user.email=ci@example.invalid",
            "commit-tree",
            &tree,
            "-m",
            "side",
        ],
    );
    fixture.write_metadata(one_override("backend_commit", json!(side)));
    assert!(error(fixture.verify()).contains("不是当前后端 HEAD 的祖先"));

    fixture.write_metadata(one_override("backend_commit", json!("f".repeat(40))));
    assert!(error(fixture.verify()).contains("Git 命令失败"));

    fixture.write_metadata(one_override("backend_commit", json!(fixture.source)));
    assert!(error(fixture.verify_with(&fixture.source, REPOSITORY)).contains("后端工作树 HEAD"));
}

#[test]
fn rejects_invalid_repository_commit_path_digest_version_and_fields() {
    let cases = [
        (
            one_override("backend_repository", json!("other/repository")),
            "后端仓库不匹配",
        ),
        (
            one_override(
                "backend_repository",
                json!("https://example.invalid/repository"),
            ),
            "owner/repository",
        ),
        (
            one_override("backend_commit", json!("A".repeat(40))),
            "小写 40 位",
        ),
        (
            one_override("openapi_path", json!("../openapi.json")),
            "路径穿越",
        ),
        (
            one_override("openapi_path", json!("openapi\\openapi.json")),
            "路径穿越",
        ),
        (
            one_override("openapi_path", json!("other/openapi.json")),
            "必须是 openapi/openapi.json",
        ),
        (one_override("sha256", json!("0".repeat(64))), "摘要不匹配"),
        (one_override("sha256", json!("A".repeat(64))), "小写 64 位"),
        (
            one_override("openapi_version", json!("3.0.0")),
            "版本与来源元数据不一致",
        ),
        (
            one_override("openapi_version", json!("2.0.0")),
            "OpenAPI 3 版本",
        ),
        (
            one_override("schema_version", json!(2)),
            "schema_version 必须为 1",
        ),
        (one_override("unexpected", json!(true)), "字段不匹配"),
    ];
    for (overrides, message) in cases {
        let fixture = SourceFixture::new();
        fixture.write_metadata(overrides);
        let actual = error(fixture.verify());
        assert!(
            actual.contains(message),
            "expected {message:?}, got {actual:?}"
        );
    }
}

#[test]
fn rejects_non_object_missing_or_wrong_typed_metadata() {
    for (document, message) in [
        (b"[]".as_slice(), "必须是对象"),
        (b"{".as_slice(), "无法读取前端正式契约来源元数据"),
    ] {
        let fixture = SourceFixture::new();
        fs::write(fixture.frontend.join("openapi/source.json"), document).unwrap();
        assert!(error(fixture.verify()).contains(message));
    }

    let fixture = SourceFixture::new();
    let mut metadata = fixture.metadata();
    metadata.remove("sha256");
    fs::write(
        fixture.frontend.join("openapi/source.json"),
        serde_json::to_vec(&metadata).unwrap(),
    )
    .unwrap();
    assert!(error(fixture.verify()).contains("字段不匹配"));

    for (key, value) in [
        ("backend_repository", json!(1)),
        ("backend_commit", json!(1)),
        ("openapi_path", json!(1)),
        ("openapi_version", json!(1)),
        ("sha256", json!(1)),
    ] {
        let fixture = SourceFixture::new();
        fixture.write_metadata(one_override(key, value));
        assert!(error(fixture.verify()).contains(&format!("{key} 必须是字符串")));
    }
}

#[test]
fn rejects_duplicate_metadata_keys_at_every_object_depth() {
    let fixture = SourceFixture::new();
    let metadata = fixture.metadata();
    let valid = serde_json::to_string(&metadata).unwrap();
    for field in ["backend_repository", "openapi_path", "sha256"] {
        let original = format!(
            r#""{field}":{}"#,
            serde_json::to_string(&metadata[field]).unwrap()
        );
        let duplicate_value = match field {
            "backend_repository" => json!("other/repository"),
            "openapi_path" => json!("../openapi.json"),
            "sha256" => json!("0".repeat(64)),
            _ => unreachable!(),
        };
        let duplicate = format!(
            r#""{field}":{}"#,
            serde_json::to_string(&duplicate_value).unwrap()
        );
        let document = valid.replacen(&original, &format!("{original},{duplicate}"), 1);
        assert_ne!(document, valid, "测试必须实际插入字段 {field} 的重复键");
        fixture.write_raw_metadata(document);
        let actual = error(fixture.verify());
        assert!(
            actual.contains(&format!("重复对象键 \"{field}\"")),
            "expected duplicate {field:?}, got {actual:?}"
        );
    }

    let nested = valid
        .strip_suffix('}')
        .map(|prefix| format!(r#"{prefix},"unexpected":[{{"scope":1,"scope":2}}]}}"#))
        .unwrap();
    fixture.write_raw_metadata(nested);
    assert!(error(fixture.verify()).contains(r#"重复对象键 "scope""#));
}

#[test]
fn rejects_invalid_expected_inputs_and_unreadable_files() {
    let fixture = SourceFixture::new();
    assert!(error(fixture.verify_with(&fixture.head, "invalid")).contains("owner/repository"));
    assert!(error(fixture.verify_with(&"A".repeat(40), REPOSITORY)).contains("小写 40 位"));

    fs::remove_file(&fixture.candidate).unwrap();
    assert!(error(fixture.verify()).contains("无法读取后端候选 OpenAPI"));

    let fixture = SourceFixture::new();
    fs::remove_file(fixture.frontend.join("openapi/source.json")).unwrap();
    assert!(error(fixture.verify()).contains("无法读取前端正式契约来源元数据"));
}

#[test]
fn rejects_invalid_utf8_non_object_or_mismatched_openapi_version() {
    for (openapi, message) in [
        (b"\xff".as_slice(), "不是有效 UTF-8 JSON"),
        (b"[]".as_slice(), "版本与来源元数据不一致"),
        (b"{\"openapi\":3}".as_slice(), "版本与来源元数据不一致"),
    ] {
        let fixture = SourceFixture::with_openapi(openapi);
        let actual = error(fixture.verify());
        assert!(
            actual.contains(message),
            "expected {message:?}, got {actual:?}"
        );
    }
}

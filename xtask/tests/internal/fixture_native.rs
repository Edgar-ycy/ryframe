use std::{
    cell::RefCell,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

use serde_json::{Value, json};

use super::{
    cli::FixtureReviewOptions,
    recovery::fixture_control::{
        review::{renew as renew_review, renew_observed},
        source_pair::publish as publish_source_pair,
    },
    workspace::root_dir,
};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    directory: PathBuf,
    backend: PathBuf,
    published:
        RefCell<Option<super::recovery::fixture_control::source_pair::execution::tests::Fixture>>,
}

impl Fixture {
    fn new() -> Self {
        let directory = root_dir().join(".local-tests").join(format!(
            "xtask-fixture-native-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        let backend = directory.join("fixture").join("backend");
        fs::create_dir_all(&backend).unwrap();
        fs::create_dir_all(backend.join(".local-tests").join("reference-fixture")).unwrap();
        let status = Command::new("git")
            .args(["init", "--quiet"])
            .arg(&backend)
            .status()
            .unwrap();
        assert!(status.success());
        Self {
            directory,
            backend,
            published: RefCell::new(None),
        }
    }

    fn write_fixture(&self) {
        write_json(
            &self.directory.join("fixture").join("fixture.json"),
            &fixture_receipt(
                &self.backend,
                &self.directory.join("fixture").join("frontend"),
            ),
        );
    }

    fn source_pair(&self) -> PathBuf {
        self.backend
            .join(".local-tests")
            .join("reference-fixture")
            .join("source-pair.json")
    }

    fn controlled(&self, name: &str) -> PathBuf {
        self.backend.join(".local-tests").join(name)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.directory.exists() {
            fs::remove_dir_all(&self.directory).unwrap();
        }
    }
}

#[test]
fn source_pair_publish_binds_fixture_and_is_create_only() {
    let fixture = Fixture::new();
    fixture.write_fixture();
    let output = fixture.source_pair();
    let receipt = publish_source_pair(&fixture.backend, &output).unwrap();
    assert_eq!(receipt["kind"], "reference-fixture-source-pair");
    assert_eq!(receipt["format_version"], 1);
    assert_eq!(
        receipt["fixture_receipt"]["path"],
        receipt_path_text(&fixture.directory.join("fixture").join("fixture.json"),)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&output).unwrap()).unwrap(),
        receipt
    );

    let error = publish_source_pair(&fixture.backend, &output).unwrap_err();
    assert!(error.to_string().contains("已存在"));
}

#[test]
fn source_pair_rejects_missing_unknown_duplicate_and_drifted_fields() {
    let fixture = Fixture::new();
    let receipt = fixture_receipt(
        &fixture.backend,
        &fixture.directory.join("fixture").join("frontend"),
    );
    let path = fixture.directory.join("fixture").join("fixture.json");

    let mut missing = receipt.clone();
    missing.as_object_mut().unwrap().remove("sources");
    write_json(&path, &missing);
    assert!(publish_source_pair(&fixture.backend, &fixture.source_pair()).is_err());

    let mut unknown = fixture_receipt(
        &fixture.backend,
        &fixture.directory.join("fixture").join("frontend"),
    );
    unknown["unexpected"] = json!(true);
    write_json(&path, &unknown);
    assert!(publish_source_pair(&fixture.backend, &fixture.source_pair()).is_err());

    let duplicate = serde_json::to_string(&receipt).unwrap();
    fs::write(&path, duplicate.replacen('{', "{\"format_version\":1,", 1)).unwrap();
    assert!(publish_source_pair(&fixture.backend, &fixture.source_pair()).is_err());

    let mut drift = receipt;
    drift["sources"]["backend"]["patch_sha256"] = json!("A".repeat(64));
    write_json(&path, &drift);
    assert!(publish_source_pair(&fixture.backend, &fixture.source_pair()).is_err());
}

#[test]
fn source_pair_rejects_out_of_bound_and_linked_outputs() {
    let fixture = Fixture::new();
    fixture.write_fixture();
    let outside = fixture.directory.join("fixture").join("source-pair.json");
    assert!(publish_source_pair(&fixture.backend, &outside).is_err());

    let allowed = fixture.backend.join(".local-tests/reference-fixture");
    let linked = fixture.backend.join(".local-tests/linked");
    create_directory_link(&allowed, &linked);
    let linked_output = linked.join("source-pair.json");
    assert!(publish_source_pair(&fixture.backend, &linked_output).is_err());
    remove_directory_link(&linked);
}

#[test]
fn review_renewal_derives_all_sides_and_is_create_only() {
    let fixture = Fixture::new();
    let execution = fixture.git_execution();
    let template = fixture.review_template(&execution);
    let template_path = fixture.controlled("template.json");
    let fixture_path = fixture.execution_source();
    write_json(&template_path, &template);
    let options = fixture.review_options(
        template_path,
        fixture_path,
        execution
            .join(".local-tests")
            .join("reference-fixture")
            .join("recovery-r11"),
        fixture.controlled("renewed-review.json"),
    );
    #[cfg(windows)]
    let options = {
        let mut selected = options;
        for path in [
            &mut selected.template,
            &mut selected.execution_source,
            &mut selected.future_root,
            &mut selected.output,
        ] {
            *path = PathBuf::from(path.to_str().unwrap().replace('\\', "/"));
        }
        selected
    };
    let summary = renew_review(&fixture.backend, &options).unwrap();
    assert_eq!(summary["status"], "renewed");
    let renewed: Value = serde_json::from_slice(&fs::read(&options.output).unwrap()).unwrap();
    let publication: Value =
        serde_json::from_slice(&fs::read(&options.execution_source).unwrap()).unwrap();
    let descriptor =
        super::recovery::clone::native::digest::file(&options.execution_source).unwrap();
    assert_eq!(
        renewed["execution_source"],
        json!({"path":receipt_path_text(&options.execution_source),
            "bytes":descriptor.bytes,"sha256":descriptor.sha256})
    );
    assert_eq!(
        renewed["sources"]["backend_head"],
        publication["original"]["source_pair"]["sources"]["backend"]["head"]
    );
    assert_ne!(
        renewed["sources"]["backend_head"],
        publication["published"]["backend"]["head"]
    );
    assert_eq!(renewed["ready_for_execution"], false);
    assert!(renewed.get("preflight").is_none());
    assert_eq!(
        renewed["scopes"]["seed"]["scope_id"],
        "fixture-seed-r11-current"
    );
    assert_eq!(
        renewed["scopes"]["candidate"]["api_url"],
        "http://127.0.0.1:18232"
    );
    assert_eq!(
        renewed["scopes"]["base"]["backend_dir"],
        receipt_path_text(&execution)
    );
    assert_eq!(
        renewed["services"]["rustfs"]["data_dir"],
        receipt_path_text(
            &execution
                .join(".local-tests")
                .join("reference-fixture")
                .join("recovery-r11")
                .join("rustfs"),
        )
    );
    assert_eq!(renewed["services"]["redis"]["endpoint"], "127.0.0.1:16391");
    assert!(
        renewed["scopes"]["seed"]["databases"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| Path::new(item["connection_file"].as_str().unwrap())
                .ends_with(Path::new("secrets").join("mysql-client.cnf")))
    );
    for side in ["seed", "base", "candidate"] {
        for item in renewed["scopes"][side]["databases"].as_array().unwrap() {
            let declared = item["connection_file"].as_str().unwrap();
            let native = Path::new(declared).components().collect::<PathBuf>();
            assert_eq!(declared, native.to_str().unwrap());
        }
    }

    let error = renew_review(&fixture.backend, &options).unwrap_err();
    assert!(error.to_string().contains("已存在"));
}

#[test]
fn review_rejects_invalid_structure_ports_and_input_drift() {
    let fixture = Fixture::new();
    let execution = fixture.git_execution();
    let template = fixture.review_template(&execution);
    let template_path = fixture.controlled("template.json");
    let fixture_path = fixture.execution_source();
    let mut missing = template.clone();
    missing["scopes"]["base"]
        .as_object_mut()
        .unwrap()
        .remove("databases");
    write_json(&template_path, &missing);
    let mut options = fixture.review_options(
        template_path.clone(),
        fixture_path.clone(),
        execution
            .join(".local-tests")
            .join("reference-fixture")
            .join("recovery-invalid"),
        fixture.controlled("invalid-review.json"),
    );
    assert!(renew_review(&fixture.backend, &options).is_err());

    write_json(&template_path, &template);
    options.api_port = options.worker_port;
    assert!(renew_review(&fixture.backend, &options).is_err());

    options.api_port = 18230;
    options.execution_source = fixture.published.borrow().as_ref().unwrap().receipt.clone();
    assert!(renew_review(&fixture.backend, &options).is_err());
    assert!(!options.output.exists());
    options.execution_source = fixture_path;
    fs::write(execution.join("src/lib.rs"), b"drifted\n").unwrap();
    let error = renew_review(&fixture.backend, &options).unwrap_err();
    assert!(error.to_string().contains("来源") || error.to_string().contains("源码"));
    assert!(!options.output.exists());
}

#[test]
fn review_rejects_template_or_execution_manifest_drift_after_derivation_without_publishing() {
    let fixture = Fixture::new();
    let execution = fixture.git_execution();
    let template = fixture.controlled("template.json");
    write_json(&template, &fixture.review_template(&execution));
    let options = fixture.review_options(
        template.clone(),
        fixture.execution_source(),
        execution.join(".local-tests/reference-fixture/recovery-observed"),
        fixture.controlled("observed-review.json"),
    );
    for file in [&template, &options.execution_source] {
        let original = fs::read(file).unwrap();
        let mut observed = false;
        let result = renew_observed(&fixture.backend, &options, || {
            observed = true;
            fs::write(file, b"{}\n").unwrap();
        });
        assert!(observed, "必须到达同次派生后的观察窗口");
        assert!(result.is_err());
        assert!(!options.output.exists());
        fs::write(file, original).unwrap();
    }
}

#[test]
fn review_rejects_out_of_bound_future_root_before_writing() {
    let fixture = Fixture::new();
    let execution = fixture.git_execution();
    let template_path = fixture.controlled("template.json");
    let fixture_path = fixture.execution_source();
    write_json(&template_path, &fixture.review_template(&execution));

    let outside = fixture.directory.join("outside-future");
    let options = fixture.review_options(
        template_path,
        fixture_path,
        outside,
        fixture.controlled("outside-review.json"),
    );
    assert!(renew_review(&fixture.backend, &options).is_err());
    assert!(!options.output.exists());
}

fn fixture_receipt(backend: &Path, frontend: &Path) -> Value {
    json!({
        "format_version": 1,
        "fixture": "business",
        "status": "ready",
        "fixture_sha256": "a".repeat(64),
        "sources": {
            "backend": clean_snapshot("b"),
            "frontend": clean_snapshot("c")
        },
        "paths": {
            "backend": path_text(backend),
            "frontend": path_text(frontend)
        },
        "generated": {
            "backend": clean_snapshot("b"),
            "frontend": clean_snapshot("c")
        }
    })
}

impl Fixture {
    fn git_execution(&self) -> PathBuf {
        let published =
            super::recovery::fixture_control::source_pair::execution::tests::Fixture::new_in(
                &self.backend.join(".local-tests"),
            );
        let value = published.publication();
        let execution = PathBuf::from(value["published"]["backend"]["path"].as_str().unwrap());
        fs::create_dir_all(execution.join(".local-tests/reference-fixture")).unwrap();
        self.published.replace(Some(published));
        execution
    }

    fn execution_source(&self) -> PathBuf {
        self.published
            .borrow()
            .as_ref()
            .unwrap()
            .output
            .join("execution-source.json")
    }

    fn review_template(&self, execution: &Path) -> Value {
        let mut scopes = serde_json::Map::new();
        for (index, side) in ["seed", "base", "candidate"].iter().enumerate() {
            let scope = format!("fixture-{side}-old");
            scopes.insert(
                (*side).into(),
                json!({
                    "scope_id": scope,
                    "runtime_dir": path_text(&self.directory.join(format!("runtime-{side}"))),
                    "backend_dir": path_text(execution),
                    "identity_ledger": path_text(&self.directory.join(format!("identities-{side}"))),
                    "api_url": format!("http://127.0.0.1:{}", 18210 + index),
                    "worker_ready_url": format!("http://127.0.0.1:{}/readyz", 19210 + index),
                    "frontend_url": format!("http://127.0.0.1:{}", 4190 + index),
                    "objects": {
                        "endpoint": "http://127.0.0.1:29200",
                        "region": "us-east-1"
                    },
                    "redis": {
                        "url": "redis://127.0.0.1:16390/0",
                        "namespace": format!("ryframe:{{{scope}}}:"),
                        "ownership_key": format!("ryframe:{{{scope}}}:.ryframe-owner"),
                        "ownership_value": "old"
                    },
                    "databases": review_databases(side),
                }),
            );
        }
        json!({
            "format_version": 1,
            "kind": "review-only-perf-resource-plan-with-readonly-preflight",
            "ready_for_execution": true,
            "reference": {
                "source": {"databases": []},
                "protected_target": {"databases": []}
            },
            "tools": {
                "mysql": {"path": "C:/tools/mysql.exe", "sha256": "a".repeat(64)},
                "mysqldump": {"path": "C:/tools/mysqldump.exe", "sha256": "b".repeat(64)},
                "rustfs": {"path": "C:/tools/rustfs.exe", "sha256": "d".repeat(64)},
                "redis_server": {
                    "distribution": "Ubuntu",
                    "resolved_path": "/usr/bin/redis-server",
                    "sha256": "e".repeat(64)
                }
            },
            "services": {
                "rustfs": {
                    "api": "http://127.0.0.1:29200",
                    "console": "http://127.0.0.1:29201",
                    "data_dir": path_text(&self.directory.join("old-rustfs"))
                },
                "redis": {"directory": path_text(&self.directory.join("old-redis"))}
            },
            "scopes": Value::Object(scopes),
        })
    }

    fn review_options(
        &self,
        template: PathBuf,
        execution_source: PathBuf,
        future_root: PathBuf,
        output: PathBuf,
    ) -> FixtureReviewOptions {
        FixtureReviewOptions {
            template,
            execution_source,
            future_root,
            id: "r11-current".into(),
            api_port: 18230,
            worker_port: 19230,
            frontend_port: 4200,
            rustfs_api_port: 29210,
            rustfs_console_port: 29211,
            redis_port: 16391,
            output,
        }
    }
}

fn review_databases(side: &str) -> Value {
    Value::Array(
        [
            "shared-control",
            "shared",
            "dedicated-a",
            "dedicated-b",
        ]
        .into_iter()
        .map(|key| {
            json!({
                "key": key,
                "database": format!("old_{side}_{}", key.replace('-', "_")),
                "mode": if matches!(key, "shared-control" | "shared") {"shared"} else {"dedicated"},
                "expected_server_uuid": "server-uuid",
                "connection_file": "C:/old.cnf",
                "host": "127.0.0.1",
                "port": 3306
            })
        })
        .collect(),
    )
}

fn clean_snapshot(head: &str) -> Value {
    json!({
        "head": head.repeat(40),
        "patch_sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        "files": []
    })
}

fn write_json(path: &Path, value: &Value) {
    fs::write(
        path,
        format!("{}\n", serde_json::to_string_pretty(value).unwrap()),
    )
    .unwrap();
}

fn path_text(path: &Path) -> String {
    path.to_str().unwrap().to_owned()
}

fn receipt_path_text(path: &Path) -> String {
    #[cfg(windows)]
    {
        path_text(path).replace('/', "\\")
    }
    #[cfg(not(windows))]
    {
        path_text(path)
    }
}

#[cfg(unix)]
fn create_directory_link(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[cfg(windows)]
fn create_directory_link(target: &Path, link: &Path) {
    let status = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "New-Item -ItemType Junction -Path $env:RYFRAME_FIXTURE_LINK -Target $env:RYFRAME_FIXTURE_TARGET | Out-Null",
        ])
        .env("RYFRAME_FIXTURE_LINK", link)
        .env("RYFRAME_FIXTURE_TARGET", target)
        .status()
        .unwrap();
    assert!(status.success());
}

#[cfg(unix)]
fn remove_directory_link(link: &Path) {
    fs::remove_file(link).unwrap();
}

#[cfg(windows)]
fn remove_directory_link(link: &Path) {
    fs::remove_dir(link).unwrap();
}

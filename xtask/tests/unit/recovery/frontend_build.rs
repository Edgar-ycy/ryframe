use super::{configuration, files, model, source, validate};
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

fn git(root: &Path, arguments: &[&str]) {
    let mut command = Command::new("git");
    command.arg("-C").arg(root).args(arguments);
    for (name, _) in std::env::vars_os() {
        if name
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GIT_")
        {
            command.env_remove(name);
        }
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// 使用真实 Git、配置和产物验证组件边界；这不是 Vite 产品构建证明。
pub(in crate::recovery) fn fixture() -> tempfile::TempDir {
    let parent = crate::workspace::root_dir().join(".local-tests/frontend-build-unit");
    fs::create_dir_all(&parent).unwrap();
    let temporary = tempfile::Builder::new()
        .prefix("中文 空格-")
        .tempdir_in(parent)
        .unwrap();
    let root = temporary.path();
    for directory in ["config", "dist/.vite", ".local-tests/build"] {
        fs::create_dir_all(root.join(directory)).unwrap();
    }
    fs::write(root.join(".gitignore"), "/dist/\n/.local-tests/\n").unwrap();
    fs::write(root.join("config/app.test.toml"),
        "api_origin=\"\"\n[server]\nhost=\"localhost\"\nport=5173\nproxy_target=\"http://localhost:8080\"\n").unwrap();
    fs::write(root.join("dist/index.html"), "fixture asset").unwrap();
    fs::write(root.join("dist/.vite/manifest.json"), "{}").unwrap();
    git(root, &["init", "--quiet"]);
    git(root, &["config", "core.autocrlf", "false"]);
    git(root, &["add", "."]);
    git(
        root,
        &[
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "-m",
            "fixture",
        ],
    );
    temporary
}

pub(in crate::recovery) fn write_receipt(root: &Path, commit: &str, inventory: &Value) {
    fs::write(
        root.join(".local-tests/build/source.json"),
        serde_json::to_vec(&json!({"format_version":1,"frontend_commit":commit})).unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("dist/build-identity.json"),
        serde_json::to_vec(&json!({"frontend_commit":commit})).unwrap(),
    )
    .unwrap();
    let inputs = configuration::capture(root, "test").unwrap();
    let domains = source::protocol::request(&json!({
        "action":"domains", "repository":"frontend", "inventory":inventory,
    }))
    .unwrap();
    let receipt = json!({"format_version":3,"kind":"restore-frontend-build",
        "sources":domains,"files":files::artifacts(root).unwrap().files,
        "build":{"command":["vite","build"],"mode":"production","target":"vite-default",
            "toolchain":{"node":"fixture","vite":"fixture",
                "pnpm":{"pinned":"fixture","observed":"fixture"}},
            "configuration":inputs.configuration,"identity":inputs.identity}});
    fs::write(
        root.join("dist").join(files::RECEIPT),
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
}

#[test]
fn inferred_and_explicit_full_inventory_accept_real_clean_source_and_reject_mismatched_head() {
    let temporary = fixture();
    let root = temporary.path();
    let inventory = source::inventory_value(&source::capture(root).unwrap());
    let head = inventory["source"]["snapshot"]["head"].as_str().unwrap();
    for commit in ["development", head] {
        write_receipt(root, commit, &inventory);
        for supplied in [None, Some(&inventory)] {
            let proof = validate(root, supplied, None).unwrap();
            assert_eq!(proof.inventory, inventory);
            proof.verify().unwrap();
        }
    }
    let wrong_head = if head == "0".repeat(40) {
        "1".repeat(40)
    } else {
        "0".repeat(40)
    };
    write_receipt(root, &wrong_head, &inventory);
    assert!(validate(root, None, None).is_err());
}

#[test]
fn held_proof_prevents_or_detects_changes_to_bound_files() {
    let temporary = fixture();
    let root = temporary.path();
    let tools = root.join(".local-tests/frontend-tools.json");
    fs::write(&tools, "{}").unwrap();
    let inventory = source::inventory_value(&source::capture(root).unwrap());
    let head = inventory["source"]["snapshot"]["head"].as_str().unwrap();
    write_receipt(root, head, &inventory);
    let config = root.join("config/app.test.toml");
    let artifact = root.join("dist/index.html");
    let mut comment = fs::read(&config).unwrap();
    comment.extend(b"# same configuration\n");
    for (path, bytes) in [
        (&tools, Some(b"{}\n".as_slice())),
        (&tools, None),
        (&artifact, Some(b"changed asset".as_slice())),
        (&config, Some(comment.as_slice())),
    ] {
        let original = fs::read(path).unwrap();
        let proof = validate(root, Some(&inventory), Some(head)).unwrap();
        let result = match bytes {
            Some(bytes) => fs::write(path, bytes),
            None => fs::remove_file(path),
        };
        if cfg!(windows) {
            assert_eq!(result.unwrap_err().raw_os_error(), Some(32));
            assert_eq!(fs::read(path).unwrap(), original);
            proof.verify().unwrap();
        } else {
            result.unwrap();
            assert!(proof.verify().is_err());
        }
        drop(proof);
        fs::write(path, original).unwrap();
    }
}

#[test]
fn held_proof_rejects_new_optional_configuration_and_dist_members() {
    let temporary = fixture();
    let root = temporary.path();
    let inventory = source::inventory_value(&source::capture(root).unwrap());
    let head = inventory["source"]["snapshot"]["head"].as_str().unwrap();
    write_receipt(root, head, &inventory);
    for name in [".local-tests/frontend-tools.json", "dist/extra.bin"] {
        let path = root.join(name);
        assert!(!path.exists());
        let proof = validate(root, Some(&inventory), Some(head)).unwrap();
        fs::write(&path, b"{}").unwrap();
        assert!(proof.verify().is_err());
        assert_eq!(fs::read(&path).unwrap(), b"{}");
        drop(proof);
        fs::remove_file(path).unwrap();
    }
    validate(root, Some(&inventory), Some(head)).unwrap();
}

#[test]
fn formal_receipt_accepts_integral_json_numbers_bound_to_real_files() {
    let temporary = fixture();
    let root = temporary.path();
    let inventory = source::inventory_value(&source::capture(root).unwrap());
    let head = inventory["source"]["snapshot"]["head"].as_str().unwrap();
    fs::write(root.join("dist/one-byte.bin"), b"x").unwrap();
    write_receipt(root, head, &inventory);

    let identity = root.join(model::IDENTITY);
    let original = format!(r#"{{"format_version":1.0,"frontend_commit":"{head}"}}"#);
    fs::write(&identity, &original).unwrap();
    let inputs = configuration::capture(root, "test").unwrap();
    assert_eq!(inputs.identity["frontend_commit"], head);
    drop(inputs);
    let receipt_path = root.join("dist").join(files::RECEIPT);
    let mut receipt = files::parse(&fs::read(&receipt_path).unwrap()).unwrap();
    receipt["build"]["identity"] = configuration::capture(root, "test").unwrap().identity;
    receipt["format_version"] = serde_json::from_str("3.0").unwrap();
    for item in receipt["files"].as_array_mut().unwrap() {
        let bytes = item["bytes"].as_u64().unwrap();
        item["bytes"] = serde_json::from_str(&format!("{bytes}.0")).unwrap();
    }
    let serialized = serde_json::to_string(&receipt).unwrap();
    assert!(serialized.contains(r#""format_version":3.0"#));
    assert!(serialized.contains(r#""bytes":1.0"#));
    fs::write(&receipt_path, &serialized).unwrap();
    let proof = validate(root, Some(&inventory), Some(head)).unwrap();
    assert_eq!(proof.receipt, receipt);
    proof.verify().unwrap();
    assert_eq!(fs::read_to_string(&receipt_path).unwrap(), serialized);
    assert_eq!(fs::read_to_string(&identity).unwrap(), original);
    drop(proof);
    for version in [
        "true",
        "false",
        "0",
        "2",
        "1.5",
        "\"1\"",
        "NaN",
        "Infinity",
        "-Infinity",
    ] {
        fs::write(
            &identity,
            format!(r#"{{"format_version":{version},"frontend_commit":"{head}"}}"#),
        )
        .unwrap();
        assert!(configuration::capture(root, "test").is_err(), "{version}");
    }
}

#[test]
fn receipt_numbers_enforce_safe_integer_boundaries() {
    let temporary = fixture();
    let root = temporary.path();
    let inventory = source::inventory_value(&source::capture(root).unwrap());
    let head = inventory["source"]["snapshot"]["head"].as_str().unwrap();
    write_receipt(root, head, &inventory);
    let original =
        files::parse(&fs::read(root.join("dist").join(files::RECEIPT)).unwrap()).unwrap();
    model::receipt(&original).unwrap();
    for version in ["true", "false", "2", "4", "3.5", "\"3\""] {
        let mut receipt = original.clone();
        receipt["format_version"] = serde_json::from_str(version).unwrap();
        assert!(model::receipt(&receipt).is_err(), "{version}");
    }
    for (bytes, expected) in [
        ("0", Some(0_u64)),
        ("-0.0", Some(0)),
        ("9007199254740991", Some(9_007_199_254_740_991)),
        ("9007199254740991.0", Some(9_007_199_254_740_991)),
        ("9007199254740992", None),
        ("9007199254740992.0", None),
        ("-1", None),
        ("1.5", None),
        ("true", None),
        ("false", None),
        ("\"1\"", None),
    ] {
        let mut receipt = original.clone();
        receipt["files"][0]["bytes"] = serde_json::from_str(bytes).unwrap();
        assert_eq!(
            model::receipt(&receipt).is_ok(),
            expected.is_some(),
            "{bytes}"
        );
        if let Some(expected) = expected {
            assert_eq!(
                model::normalized_files(&receipt["files"]).unwrap()[0]["bytes"],
                expected
            );
        }
    }
    for value in ["NaN", "Infinity", "-Infinity"] {
        let receipt = serde_json::to_string(&original).unwrap();
        let invalid = receipt.replace(
            r#""format_version":3"#,
            &format!(r#""format_version":{value}"#),
        );
        assert!(files::parse(invalid.as_bytes()).is_err(), "{value}");
    }
}

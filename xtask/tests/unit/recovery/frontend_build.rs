use super::{artifact_snapshot, b0, configuration, files, model, source, validate};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[path = "frontend_build_configuration_cases.rs"]
mod configuration_cases;

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
        if commit == "development" {
            assert!(validate(root, Some(&inventory), Some(head)).is_err());
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
fn readonly_frontend_proof_preserves_every_input_and_artifact() {
    let temporary = fixture();
    let root = temporary.path();
    fs::write(root.join(".local-tests/frontend-tools.json"), b"{}").unwrap();
    let inventory = source::inventory_value(&source::capture(root).unwrap());
    let head = inventory["source"]["snapshot"]["head"].as_str().unwrap();
    write_receipt(root, head, &inventory);
    let before = fixture_contents(root);
    let proof = validate(root, Some(&inventory), Some(head)).unwrap();
    proof.verify().unwrap();
    assert_eq!(fixture_contents(root), before);
    drop(proof);
    fs::write(
        root.join("dist/build-identity.json"),
        serde_json::to_vec(&json!({"frontend_commit":"development"})).unwrap(),
    )
    .unwrap();
    let error = validate(root, Some(&inventory), Some(head))
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("身份"), "{error}");
}

fn fixture_contents(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    let mut contents = BTreeMap::new();
    let mut pending = vec![root.to_owned()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path == root.join(".git") {
                continue;
            }
            let kind = entry.file_type().unwrap();
            let bytes = if kind.is_dir() {
                pending.push(path.clone());
                None
            } else {
                assert!(kind.is_file());
                Some(fs::read(&path).unwrap())
            };
            contents.insert(path, bytes);
        }
    }
    contents
}

#[test]
fn snapshot_orders_unicode_assets_and_requires_manifest() {
    let temporary = fixture();
    let root = temporary.path();
    for name in ["z.txt", "中文.txt", "a.txt", "😀.txt"] {
        fs::write(root.join("dist").join(name), name).unwrap();
    }
    let snapshot = artifact_snapshot(root).unwrap();
    let names = snapshot
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["path"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            ".vite/manifest.json",
            "a.txt",
            "index.html",
            "z.txt",
            "中文.txt",
            "😀.txt"
        ]
    );
    fs::remove_file(root.join("dist/.vite/manifest.json")).unwrap();
    let error = artifact_snapshot(root).unwrap_err().to_string();
    assert!(error.contains("manifest.json"), "{error}");
}

#[cfg(any(unix, windows))]
#[test]
fn frontend_proof_rejects_linked_dist_and_dangling_selected_toml() {
    let temporary = fixture();
    let root = temporary.path();
    let selected = root.join("config/app.test.toml");
    let linked = root.join("dist/linked.toml");
    #[cfg(unix)]
    use std::os::unix::fs::symlink as link;
    #[cfg(windows)]
    use std::os::windows::fs::symlink_file as link;
    link(&selected, &linked).unwrap();
    let error = artifact_snapshot(root).unwrap_err().to_string();
    assert!(error.contains("dist 禁止链接"), "{error}");
    fs::remove_file(linked).unwrap();
    fs::remove_file(&selected).unwrap();
    let missing = root.join("config/missing.toml");
    link(missing, selected).unwrap();
    let error = configuration::capture(root, "test")
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("链接"), "{error}");
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

#[test]
fn b0_receipt_accepts_frozen_adapter_sources_and_rejects_each_changed_digest() {
    let temporary = fixture();
    let receipt = frozen_b0_receipt(temporary.path());
    b0::receipt(&receipt).unwrap();
    for index in 0..3 {
        let mut changed = receipt.clone();
        changed["adapter"]["sources"][index]["sha256"] = json!("0".repeat(64));
        let error = b0::receipt(&changed).unwrap_err().to_string();
        assert_eq!(error, "必须使用精确 B0 前端适配收据");
    }
    b0::receipt(&receipt).unwrap();
}

// 固定历史来源独立于当前适配器；这是完整收据格式回归，不是真实构建产物证明。
fn frozen_b0_receipt(root: &Path) -> Value {
    let digest = "a".repeat(64);
    let inventory = json!({
        "source":{"snapshot":{"head":b0::HEAD,"patch_sha256":digest,"files":[],"clean":true},
            "worktree_fingerprint":format!("sha256:{digest}")},
        "files":[{"path":"src/main.ts","sha256":digest}],
        "guard":{"head":b0::HEAD,"index_sha256":digest,"modes_sha256":digest},
    });
    let domains = source::protocol::request(&json!({
        "action":"domains","repository":"frontend","inventory":inventory,
    }))
    .unwrap();
    let program = json!({"path":root.join("dist/index.html"),"bytes":1,"sha256":digest});
    let inputs = [
        (
            ".env",
            "d24709d7deea3cc0012bb08780bba0a9f8190d47b0335c4dd8a4a32b8050ec64",
        ),
        (
            ".node-version",
            "b911698dc3cf3227d34d0ad6727f1c4408791203c9665de99ab959331a1fb07d",
        ),
        ("package.json", digest.as_str()),
        ("pnpm-lock.yaml", digest.as_str()),
        ("pnpm-workspace.yaml", digest.as_str()),
        ("vite.config.ts", digest.as_str()),
    ]
    .map(|(path, sha256)| json!({"path":path,"sha256":sha256}));
    let dependency_state = [
        "node_modules/.modules.yaml",
        "node_modules/.pnpm/lock.yaml",
        "node_modules/.pnpm-workspace-state-v1.json",
        if cfg!(windows) {
            "node_modules/.bin/vite.CMD"
        } else {
            "node_modules/.bin/vite"
        },
    ]
    .map(|path| json!({"path":path,"sha256":digest}));
    let artifacts = [".vite/manifest.json", "build-identity.json", "index.html"]
        .map(|path| json!({"path":path,"bytes":1,"sha256":digest}));
    json!({
        "format_version":1,"kind":b0::KIND,
        "adapter":{"contract":b0::CONTRACT,"program":program,"sources":[
            {"path":"xtask/src/recovery/frontend_build/b0.rs",
                "sha256":"285ac9f7497f5ad2c79709c9523ffba70defd2ba6bb88085aec46413e48a7f04"},
            {"path":"xtask/src/recovery/frontend_build/b0/inputs.rs",
                "sha256":"93ceced5d02d1372cf3c5a6e64f99d254b08e4ac39ebce6bef4c404c0ca9754b"},
            {"path":"xtask/src/recovery/frontend_build/b0/toolchain.rs",
                "sha256":"c75d9e52645e8b893aea4e237f87ec95d4d2922fbfc960f5eced738389aca163"},
        ]},
        "sources":domains,
        "build":{"command":["corepack","pnpm","build"],"mode":"production",
            "identity":{"frontend_commit":b0::HEAD,"source_file":null},
            "configuration":{"api_origin":"same-origin",
                "frontend_tree":"52fed4e1321ab4b4c04839a3a165c6bd1645e4b6",
                "inputs":inputs,"dependency_state":dependency_state},
            "toolchain":b0_toolchain(&program, root)},
        "files":artifacts,
    })
}

fn b0_toolchain(program: &Value, root: &Path) -> Value {
    json!({
        "node":{"version":"v24.15.0","file":program},
        "corepack":{"version":"0.34.6","package":program,"entry":program,"implementation":program},
        "pnpm":{"version":"11.20.0","home":root,"package":program,"entry":program,
            "implementation":program,"installation":program},
        "vite":{"version":"8.1.5","package":program,"entry":program},
    })
}

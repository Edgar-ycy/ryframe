//! 真实 Git 对象及绑定文件验证独立风险；模型夹具不作为正式收据接受证据。
use super::{adapter, arms, common, digest, document, published, shape};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn git(root: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    command.current_dir(root).args(args);
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
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn directory() -> tempfile::TempDir {
    let parent = crate::workspace::root_dir().join(".local-tests/comparison-source-unit");
    fs::create_dir_all(&parent).unwrap();
    tempfile::tempdir_in(parent).unwrap()
}

fn imported(parent: &Path, name: &str, commit: &str) -> PathBuf {
    let root = parent.join(name);
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "--quiet"]);
    #[cfg(windows)]
    git(&root, &["config", "--local", "core.longpaths", "true"]);
    // 真实浅取固定对象；不用 alternates，不写原工作树的 index 或对象。
    git(
        &root,
        &[
            "fetch",
            "--quiet",
            "--no-tags",
            "--depth=1",
            crate::workspace::root_dir().to_str().unwrap(),
            commit,
        ],
    );
    git(
        &root,
        &[
            "-c",
            "core.autocrlf=false",
            "checkout",
            "--quiet",
            "--detach",
            commit,
        ],
    );
    root
}

fn files(root: &Path) -> BTreeMap<PathBuf, (String, std::time::SystemTime)> {
    let mut result = BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            result.extend(files(&path));
        } else {
            result.insert(
                path.clone(),
                (
                    digest::file(&path).unwrap().sha256,
                    fs::metadata(&path).unwrap().modified().unwrap(),
                ),
            );
        }
    }
    result
}

#[test]
fn b0_real_git_reconstruction_uses_compiled_patch_and_readonly_preserves_objects() {
    use crate::recovery::b0;
    let temporary = directory();
    let root = imported(temporary.path(), "product", b0::BACKEND);
    assert!(!root.join(b0::PATCH).exists());
    let before = files(&root.join(".git"));
    let readonly = adapter::evidence(&root, false).unwrap();
    assert_eq!(files(&root.join(".git")), before);
    assert!(!root.join("target").exists());
    let rebuilt = adapter::evidence(&root, true).unwrap();
    assert_eq!(rebuilt, readonly);
    assert_eq!(rebuilt["reference_adapter_sha"], b0::COMMIT);
    assert_eq!(fs::read_dir(root.join("target")).unwrap().count(), 0);
    let after = files(&root.join(".git"));
    assert_eq!(adapter::evidence(&root, false).unwrap(), readonly);
    assert_eq!(files(&root.join(".git")), after);
    assert!(b0::validate_patch(&b0::PATCH_BYTES[..b0::PATCH_LENGTH - 1]).is_err());
    let mut changed = b0::PATCH_BYTES.to_vec();
    changed[0] ^= 1;
    assert!(b0::validate_patch(&changed).is_err());
    assert_eq!(files(&root.join(".git")), after);
    temporary.close().unwrap();
}

#[test]
fn exact_b0_roots_reject_role_swaps_replace_refs_and_product_drift() {
    use crate::recovery::b0;
    let temporary = directory();
    let base = imported(temporary.path(), "base", b0::BACKEND);
    let execution = imported(temporary.path(), "execution", b0::COMMIT);
    let coordinator = temporary.path().join("coordinator");
    fs::create_dir(&coordinator).unwrap();
    git(&coordinator, &["init", "--quiet"]);
    let before = files(&coordinator.join(".git"));
    let bound = adapter::registered(&coordinator, &execution, &base, false).unwrap();
    assert_eq!(
        bound["inventory"],
        arms::inventory(&base, Some(b0::BACKEND)).unwrap()
    );
    assert_eq!(
        bound["execution_inventory"],
        arms::inventory(&execution, Some(b0::COMMIT)).unwrap()
    );
    assert_ne!(bound["inventory"], bound["execution_inventory"]);
    assert!(adapter::registered(&coordinator, &base, &execution, false).is_err());
    assert!(adapter::registered(&coordinator, &base, &base, false).is_err());
    assert_eq!(files(&coordinator.join(".git")), before);
    for (root, original, substitute) in [
        (&base, b0::BACKEND, b0::COMMIT),
        (&execution, b0::COMMIT, b0::BACKEND),
    ] {
        git(
            root,
            &[
                "fetch",
                "--quiet",
                "--no-tags",
                "--depth=1",
                crate::workspace::root_dir().to_str().unwrap(),
                substitute,
            ],
        );
        git(root, &["replace", original, substitute]);
        assert!(adapter::registered(&coordinator, &execution, &base, false).is_err());
        git(root, &["replace", "-d", original]);
    }
    assert_eq!(
        adapter::registered(&coordinator, &execution, &base, false).unwrap(),
        bound
    );
    fs::write(execution.join("Cargo.toml"), "changed product input").unwrap();
    assert!(adapter::registered(&coordinator, &execution, &base, false).is_err());
    temporary.close().unwrap();
}

fn binding(path: &Path) -> Value {
    json!({"path":path,"bytes":1,"sha256":"a".repeat(64)})
}

fn manifest(root: &Path) -> Value {
    let inventory = json!({"source":{"snapshot":{"head":"b".repeat(40),"patch_sha256":"c".repeat(64),"files":[],"clean":true},
        "worktree_fingerprint":format!("sha256:{}","d".repeat(64))},
        "files":[{"path":"Cargo.toml","sha256":"e".repeat(64)}],
        "guard":{"head":"b".repeat(40),"index_sha256":"f".repeat(64),"modes_sha256":"a".repeat(64)}});
    let descriptor = binding(&root.join("result.json"));
    let value = json!({"origin_attempt":7,"source_registration":descriptor,"source_rebind":descriptor,
        "source_generation":descriptor,"review_successor":descriptor,"source_request":descriptor,"export":descriptor,
        "source_storage":{"private":"must not escape"},"summary":{"export_sha256":"a".repeat(64),
        "generation_sha256":"b".repeat(64),"logical_inventory_sha256":"c".repeat(64)}});
    let export = published::projection(&descriptor, &value).unwrap();
    let arm = |b0: bool| {
        json!({"roots":{"source_backend":root.join(if b0 {"base"} else {"candidate"}),
        "execution_backend":root.join(if b0 {"adapter"} else {"candidate"}),"frontend":root.join(if b0 {"front0"} else {"front1"})},
        "sources":{"backend":inventory,"frontend":inventory},"execution_sources":{"backend":inventory,"frontend":inventory},
        "builds":{"backend":descriptor,"frontend":descriptor},"backend_environment":descriptor,"adapter":if b0 {crate::recovery::b0::expected()} else {Value::Null},
        "source_export_result_sha256":export["result"]["sha256"],"source_export_identity_sha256":export["identity_sha256"]})
    };
    json!({"format_version":2,"kind":"restore-comparison-sources","source_export":export,"arms":{"b0":arm(true),"b1":arm(false)}})
}

#[test]
fn strict_manifest_rejects_shared_export_substitution_missing_adapter_and_unknown_fields() {
    let temporary = directory();
    let value = manifest(temporary.path());
    shape::validate(&value).unwrap();
    assert!(value["source_export"].get("source_storage").is_none());
    for (pointer, replacement) in [
        ("/arms/b0/adapter", Value::Null),
        ("/arms/b0/adapter/adapter_tree", json!("f".repeat(40))),
        ("/arms/b0/adapter/reconstructed_tree", json!("f".repeat(40))),
        (
            "/arms/b0/adapter/reference_adapter_sha",
            json!(crate::recovery::b0::BACKEND),
        ),
        ("/arms/b0/adapter/patch/bytes", json!(1)),
        ("/arms/b0/adapter/patch/sha256", json!("f".repeat(64))),
        ("/arms/b0/adapter/patch/path", json!("unregistered.patch")),
        (
            "/arms/b0/source_export_identity_sha256",
            json!("f".repeat(64)),
        ),
        ("/source_export/origin_attempt", json!(true)),
        ("/format_version", json!(true)),
        ("/format_version", json!(1)),
        ("/arms/b1/backend_environment", Value::Null),
    ] {
        let mut invalid = value.clone();
        *invalid.pointer_mut(pointer).unwrap() = replacement;
        assert!(shape::validate(&invalid).is_err(), "{pointer}");
    }
    let mut invalid = value.clone();
    invalid["source_export"]["source_storage"] = json!({"private":"unexpected"});
    assert!(shape::validate(&invalid).is_err());
    let mut invalid = value;
    invalid["source_export"]["identity_sha256"] = json!("f".repeat(64));
    assert!(shape::validate(&invalid).is_err());
}

#[test]
fn comparison_rpc_requires_explicit_matching_write_authority_and_bound_export_bytes() {
    for (read_only, write) in [(true, true), (false, false)] {
        let request = serde_json::to_vec(&json!({"operation":"comparison-verify",
            "arguments":{"read_only":read_only},"write":write}))
        .unwrap();
        let error = super::super::execute(&request).unwrap_err().to_string();
        assert!(error.contains("写入授权"), "{error}");
    }
    let temporary = directory();
    let root = temporary.path();
    fs::create_dir(root.join(".local-tests")).unwrap();
    let receipt = root.join(".local-tests/export.json");
    document::write_new(&receipt, &json!({"review_successor":null})).unwrap();
    let descriptor = common::binding(&receipt).unwrap();
    fs::write(&receipt, "{}").unwrap();
    assert!(published::identity(root, &descriptor).is_err());
    assert_eq!(fs::read_to_string(&receipt).unwrap(), "{}");
}

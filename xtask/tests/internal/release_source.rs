use std::{fs, path::Path, process::Command};

use serde_json::json;

use super::release::{
    source::{
        RepositoryRef, annotated_tag_notes, normalize_markdown, release_identity, release_manifest,
        validate_frontend_contract_source, validate_openapi, validate_repository_ref,
    },
    support::sha256,
};

fn git(repository: &Path, arguments: &[&str]) {
    assert!(
        Command::new("git")
            .args(arguments)
            .current_dir(repository)
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn canonical_identity_and_markdown_follow_release_rules() {
    assert_eq!(
        release_identity("v1.2.3").unwrap(),
        ("v1.2.3".to_owned(), "1.2.3".to_owned())
    );
    for invalid in ["1.2.3", "v01.2.3", "v1.2.3-rc.1", "v1.2"] {
        assert!(release_identity(invalid).is_err(), "{invalid}");
    }
    assert_eq!(
        normalize_markdown("\r\nfirst  \r\n\r\nsecond\t\r\n\r\n"),
        "first\n\nsecond"
    );
}

#[test]
fn annotated_tag_commit_binding_rejects_wrong_or_lightweight_tags() {
    let root = std::env::temp_dir().join(format!("ryframe-release-source-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("README.md"), "fixture\n").unwrap();
    git(&root, &["init", "--quiet"]);
    git(&root, &["add", "README.md"]);
    git(
        &root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "fixture",
        ],
    );
    git(&root, &["tag", "-a", "v0.13.0", "-m", "Release notes"]);
    let commit = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&root)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_owned();
    let reference = validate_repository_ref(&root, "v0.13.0", &commit, "backend").unwrap();
    assert_eq!(reference.commit, commit);
    assert!(validate_repository_ref(&root, "v0.13.0", &"b".repeat(40), "backend").is_err());
    assert_eq!(
        annotated_tag_notes(&root, "v0.13.0", "backend").unwrap(),
        "Release notes"
    );
    git(&root, &["tag", "v0.13.1"]);
    assert!(annotated_tag_notes(&root, "v0.13.1", "backend").is_err());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn openapi_version_and_contract_source_bind_exact_backend_commit() {
    let root =
        std::env::temp_dir().join(format!("ryframe-release-contract-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("openapi")).unwrap();
    let contract = br#"{"info":{"version":"1.2.3"}}"#;
    fs::write(root.join("openapi/openapi.json"), contract).unwrap();
    let hash = validate_openapi(&root.join("openapi/openapi.json"), "1.2.3", "frontend").unwrap();
    assert_eq!(hash, sha256(contract));
    assert!(validate_openapi(&root.join("openapi/openapi.json"), "1.2.4", "frontend").is_err());
    fs::write(
        root.join("openapi/source.json"),
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "backend_repository": "owner/backend",
            "backend_commit": "a".repeat(40),
            "openapi_path": "openapi/openapi.json",
            "openapi_version": "3.1.0",
            "sha256": hash,
        }))
        .unwrap(),
    )
    .unwrap();
    validate_frontend_contract_source(&root, "owner/backend", &"a".repeat(40), &hash).unwrap();
    assert!(
        validate_frontend_contract_source(&root, "owner/backend", &"b".repeat(40), &hash).is_err()
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn release_manifest_rejects_divergent_openapi_hashes() {
    let identity = ("v0.13.0".to_owned(), "0.13.0".to_owned());
    let backend = RepositoryRef {
        repository: "owner/backend".to_owned(),
        tag_object: "b".repeat(40),
        commit: "a".repeat(40),
    };
    let frontend = RepositoryRef {
        repository: "owner/frontend".to_owned(),
        tag_object: "d".repeat(40),
        commit: "c".repeat(40),
    };
    let hash = "e".repeat(64);
    let manifest = release_manifest(&identity, &backend, &frontend, &hash, &hash).unwrap();
    assert_eq!(manifest["backend"]["commit"], "a".repeat(40));
    assert_eq!(manifest["frontend"]["version"], "0.13.0");
    assert!(release_manifest(&identity, &backend, &frontend, &hash, &"f".repeat(64)).is_err());
}

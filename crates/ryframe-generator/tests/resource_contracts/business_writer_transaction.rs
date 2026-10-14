use ryframe_generator::ResourceError;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

#[path = "../../src/business/writer/staged.rs"]
mod staged;
use super::resource_writer_transaction::transaction;

const OWNERSHIP_PATH: &str = ".ryframe/generated.toml";
#[derive(Clone)]
struct BusinessAsset {
    resource: String,
    path: String,
    content: String,
}
fn content_hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
use content_hash as hash;
fn error(message: impl Into<String>, path: &Path) -> ResourceError {
    ResourceError::new(message, "保留并核对事务现场").with_file(path.to_string_lossy())
}

fn commit_stage(
    root: &Path,
    stage: tempfile::TempDir,
    assets: &[BusinessAsset],
    paths: &BTreeSet<&str>,
    expected: &BTreeMap<String, transaction::ExpectedFile>,
    manifest: &str,
) -> Result<(), ResourceError> {
    let mut all_assets = assets.to_vec();
    all_assets.push(BusinessAsset {
        resource: "__catalog__".into(),
        path: OWNERSHIP_PATH.into(),
        content: manifest.into(),
    });
    staged::commit_files(root, stage, &all_assets, paths, expected)
}

fn fixture() -> (
    tempfile::TempDir,
    Vec<BusinessAsset>,
    BTreeMap<String, transaction::ExpectedFile>,
) {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("src/generated")).unwrap();
    fs::create_dir(root.path().join(".ryframe")).unwrap();
    fs::write(root.path().join("src/generated/a.rs"), "old-a").unwrap();
    fs::write(root.path().join(OWNERSHIP_PATH), "old-manifest").unwrap();
    let assets = ["a", "b"]
        .into_iter()
        .map(|name| BusinessAsset {
            resource: name.into(),
            path: format!("src/generated/{name}.rs"),
            content: format!("new-{name}"),
        })
        .collect();
    let expected = BTreeMap::from([
        (
            "src/generated/a.rs".into(),
            transaction::ExpectedFile::ContentHash(hash(b"old-a")),
        ),
        (
            "src/generated/b.rs".into(),
            transaction::ExpectedFile::Absent,
        ),
        (
            OWNERSHIP_PATH.into(),
            transaction::ExpectedFile::ExactBytes(b"old-manifest".to_vec()),
        ),
    ]);
    (root, assets, expected)
}

#[test]
fn missing_second_staged_file_rolls_back_first_install_and_manifest() {
    let (root, assets, expected) = fixture();
    let stage = tempfile::tempdir_in(root.path()).unwrap();
    transaction::write_staged(&stage.path().join(&assets[0].path), &assets[0].content, "a")
        .unwrap();
    transaction::write_staged(
        &stage.path().join(OWNERSHIP_PATH),
        "new-manifest",
        "catalog",
    )
    .unwrap();
    let paths = BTreeSet::from([
        OWNERSHIP_PATH,
        assets[0].path.as_str(),
        assets[1].path.as_str(),
    ]);
    commit_stage(
        root.path(),
        stage,
        &assets,
        &paths,
        &expected,
        "new-manifest",
    )
    .unwrap_err();
    assert_eq!(
        fs::read_to_string(root.path().join(&assets[0].path)).unwrap(),
        "old-a"
    );
    assert_eq!(
        fs::read_to_string(root.path().join(OWNERSHIP_PATH)).unwrap(),
        "old-manifest"
    );
    assert!(!root.path().join(&assets[1].path).exists());
}

#[test]
fn concurrent_create_is_preserved_and_previous_backups_are_restored() {
    let (root, assets, expected) = fixture();
    let stage = tempfile::tempdir_in(root.path()).unwrap();
    for asset in &assets {
        transaction::write_staged(
            &stage.path().join(&asset.path),
            &asset.content,
            &asset.resource,
        )
        .unwrap();
    }
    transaction::write_staged(
        &stage.path().join(OWNERSHIP_PATH),
        "new-manifest",
        "catalog",
    )
    .unwrap();
    fs::write(root.path().join(&assets[1].path), "concurrent-source").unwrap();
    let paths = BTreeSet::from([
        OWNERSHIP_PATH,
        assets[0].path.as_str(),
        assets[1].path.as_str(),
    ]);
    let failure = commit_stage(
        root.path(),
        stage,
        &assets,
        &paths,
        &expected,
        "new-manifest",
    )
    .unwrap_err();
    assert!(failure.to_string().contains("compare-and-swap"));
    assert_eq!(
        fs::read_to_string(root.path().join(&assets[0].path)).unwrap(),
        "old-a"
    );
    assert_eq!(
        fs::read_to_string(root.path().join(&assets[1].path)).unwrap(),
        "concurrent-source"
    );
    assert_eq!(
        fs::read_to_string(root.path().join(OWNERSHIP_PATH)).unwrap(),
        "old-manifest"
    );
}

#[test]
fn changed_owned_file_fails_before_any_staging_or_mutation() {
    let (root, assets, expected) = fixture();
    fs::write(root.path().join(&assets[0].path), "manual-edit").unwrap();
    staged::apply_changes(root.path(), &assets, &[], "new-manifest", &expected).unwrap_err();
    assert_eq!(
        fs::read_to_string(root.path().join(&assets[0].path)).unwrap(),
        "manual-edit"
    );
    assert_eq!(
        fs::read_to_string(root.path().join(OWNERSHIP_PATH)).unwrap(),
        "old-manifest"
    );
    assert!(!root.path().join(&assets[1].path).exists());
    assert!(!fs::read_dir(root.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".ryframe-stage-")
    }));
}

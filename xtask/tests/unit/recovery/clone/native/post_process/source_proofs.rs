use std::fs;

use serde_json::json;

use crate::recovery::clone::native::target_verify::guard::Inputs;

use super::*;

struct Fixture {
    _temp: tempfile::TempDir,
    backend: PathBuf,
    root: PathBuf,
    request: PathBuf,
    proof: PathBuf,
    export: Value,
    proofs: Value,
}

impl Fixture {
    fn new() -> Self {
        let workspace = crate::workspace::root_dir();
        let local = workspace.join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        let temp = tempfile::Builder::new()
            .prefix("post-copy-held-source-proofs-")
            .tempdir_in(local)
            .unwrap();
        let backend = temp.path().to_owned();
        let local = backend.join(".local-tests");
        let root = local.join("export");
        let request = local.join("source-request.json");
        let directory = root.join("objects/fixture");
        fs::create_dir_all(&directory).unwrap();
        let proof = directory.join("capture.json");
        fs::write(&proof, b"{}\n").unwrap();
        fs::write(&request, b"{}\n").unwrap();
        fs::write(directory.join("object.bin"), b"body").unwrap();
        let export = root.join("export.json");
        document::write_new(
            &export,
            &json!({"artifact_root":root,"request":common::binding(&request).unwrap()}),
        )
        .unwrap();
        let proofs = Map::from_iter([&request, &proof].map(|path| {
            let descriptor = common::binding(path).unwrap();
            (
                path.to_string_lossy().into_owned(),
                json!({"bytes":descriptor["bytes"],"sha256":descriptor["sha256"]}),
            )
        }));
        Self {
            _temp: temp,
            backend,
            root,
            request,
            proof,
            export: common::binding(&export).unwrap(),
            proofs: Value::Object(proofs),
        }
    }

    fn hold(&self) -> Result<(HeldSourceProofs, Value)> {
        HeldSourceProofs::open(&self.backend, &self.export, self.proofs.clone())
    }
}

#[test]
fn held_seed_modes_require_a_windows_seed_runtime_from_source_to_seed() {
    let source_to_seed = json!("source_to_seed");
    let seed_to_arm = json!("seed_to_arm");
    for mode in [
        "departments-plan",
        "departments-apply",
        "departments-verify",
        "identities-verify",
        "start",
        "close",
    ] {
        assert_eq!(
            supports("seed-runtime", mode, &source_to_seed),
            cfg!(windows),
        );
        assert!(!supports("seed-runtime", mode, &seed_to_arm));
        assert!(!supports("post-copy", mode, &source_to_seed));
    }
    for mode in [
        "departments-reconcile",
        "identities-apply",
        "identities-reconcile",
        "prepare",
        "stop",
    ] {
        assert!(!supports("seed-runtime", mode, &source_to_seed));
    }
}

#[test]
fn held_source_proofs_protect_the_exact_files_and_preserve_the_root_failure_marker() {
    let fixture = Fixture::new();
    let writer = fs::OpenOptions::new()
        .write(true)
        .open(&fixture.proof)
        .unwrap();
    assert!(fixture.hold().is_err());
    drop(writer);

    let (held, outside) = fixture.hold().unwrap();
    let outside = outside.as_object().unwrap();
    assert_eq!(outside.len(), 1);
    assert!(outside.contains_key(fixture.request.to_str().unwrap()));
    let ordinary = Inputs::open([fixture.request.clone()]).unwrap();
    let directory = fixture.proof.parent().unwrap();
    assert_eq!(fs::read(directory.join("object.bin")).unwrap(), b"body");
    assert!(fs::write(&fixture.proof, b"bad").is_err());
    let export_path = fixture.root.join("export.json");
    assert!(fs::write(&export_path, b"bad").is_err());
    assert!(fs::write(&fixture.request, b"bad").is_err());
    assert!(fs::rename(&fixture.proof, fixture.proof.with_extension("old")).is_err());
    assert!(fs::rename(directory, directory.with_extension("old")).is_err());
    assert!(fs::rename(&fixture.root, fixture.root.with_extension("old")).is_err());
    held.checkpoint().unwrap();
    ordinary.check().unwrap();

    let alias = directory.join("alias.json");
    if fs::hard_link(&fixture.proof, &alias).is_ok() {
        assert!(held.checkpoint().is_err());
        assert!(held.check_end().is_err());
        fs::remove_file(alias).unwrap();
    }
    let failure = fixture.root.join("failure.json");
    fs::write(&failure, b"{}").unwrap();
    assert!(held.checkpoint().is_err());
    assert!(held.check_end().is_err());
    fs::remove_file(failure).unwrap();
    held.check_end().unwrap();
    drop(held);
    assert!(fs::OpenOptions::new().write(true).open(export_path).is_ok());
    fs::write(&fixture.proof, b"bad").unwrap();
    assert!(fixture.hold().is_err());
}

#[test]
fn held_source_proofs_allow_identity_ledger_publication_in_the_same_run() {
    let fixture = Fixture::new();
    let run = fixture.backend.join(".local-tests/run");
    let copy = run.join("copy/ledger");
    let identities = run.join("seed-runtime/identities");
    fs::create_dir_all(&copy).unwrap();
    fs::create_dir_all(&identities).unwrap();

    let head = copy.join("head.json");
    let plan = run.join("seed-runtime/identity-plan.json");
    let ledger = identities.join("ledger.json");
    let prepared = json!({"status":"prepared"});
    document::write_new(&head, &json!({"sequence":4})).unwrap();
    document::write_new(&plan, &json!({"kind":"devex-identities"})).unwrap();
    document::write_new(&ledger, &prepared).unwrap();

    let (held, _) = fixture.hold().unwrap();
    let protected = Inputs::open([fixture.request.clone(), head.clone(), plan]).unwrap();
    held.checkpoint().unwrap();
    assert!(document::write_replace(&head, &json!({"sequence":5})).is_err());

    document::write_replace(&ledger, &json!({"status":"verifying"})).unwrap();
    let (intent_name, receipt_name) =
        crate::data::performance_identities::seed_evidence::names(&"a".repeat(64), &identities);
    let receipt = json!({"status":"verified"});
    document::publish_new(
        &identities.join(intent_name),
        &json!({"receipt_file":receipt_name.as_str()}),
        "身份核验测试发布意图",
    )
    .unwrap();
    document::publish_new(
        &identities.join(&receipt_name),
        &receipt,
        "身份核验测试收据",
    )
    .unwrap();
    let finished = json!({"status":"verified","verified_receipt":receipt_name});
    document::write_replace(&ledger, &finished).unwrap();

    assert_eq!(document::read(&ledger).unwrap(), finished);
    assert_eq!(
        document::read(&identities.join(receipt_name)).unwrap(),
        receipt
    );
    held.check_end().unwrap();
    protected.check().unwrap();
}

#[test]
fn held_source_proofs_bind_the_original_bytes_at_both_stage_boundaries() {
    let mut fixture = Fixture::new();
    let key = fixture.proof.to_str().unwrap().to_owned();
    let original = fixture.proofs[&key]["sha256"].clone();
    fixture.proofs[&key]["sha256"] = json!("0".repeat(64));
    assert!(fixture.hold().is_err());
    fixture.proofs[&key]["sha256"] = original;

    let (mut held, _) = fixture.hold().unwrap();
    held.checkpoint().unwrap();
    held.check_end().unwrap();
    held.proofs.get_mut(&key).unwrap()["sha256"] = json!("0".repeat(64));
    held.checkpoint().unwrap();
    assert!(
        held.check_end()
            .unwrap_err()
            .to_string()
            .contains("持有期间原来源证明文件字节或摘要变化")
    );
}

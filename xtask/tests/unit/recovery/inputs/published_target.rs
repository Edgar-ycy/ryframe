use super::*;
use crate::recovery::{
    clone::native::target_verify::input::request::{self, SourceObserver, SystemSourceObserver},
    fixture_control::source_pair::execution::tests::Fixture,
};
use serde_json::json;
use std::fs;

fn selected(fixture: &Fixture) -> Value {
    json!({"path":fixture.output.join("backend"),"execution_source":fixture.binding()})
}

#[test]
fn published_target_consumers_bind_the_actual_clean_commit_and_reject_drift() {
    let fixture = Fixture::new();
    let publication = fixture.publication();
    let root = fixture.output.join("backend");
    let declared = selected(&fixture);
    for coordinator in [&fixture.backend, &root] {
        let expected = execution_backend(coordinator, Some(&declared)).unwrap();
        let actual =
            request::execution_backend(coordinator, Some(&declared), &SystemSourceObserver)
                .unwrap();
        assert_eq!(expected, actual);
        assert_eq!(actual.1["kind"], "published-business-fixture");
        assert_eq!(
            actual.1["fixture"],
            publication["original"]["fixture_receipt"]
        );
        assert_eq!(
            actual.1["source"]["head"],
            publication["published"]["backend"]["head"]
        );
        assert_ne!(
            actual.1["source"]["head"],
            publication["original"]["source_pair"]["sources"]["backend"]["head"]
        );
        assert!(actual.1["source"].get("clean").is_none());
    }
    let source = root.join("src/lib.rs");
    let bytes = fs::read(&source).unwrap();
    fs::write(&source, b"dirty published source").unwrap();
    assert!(execution_backend(&root, Some(&declared)).is_err());
    assert!(
        std::process::Command::new("git")
            .args(["add", "src/lib.rs"])
            .current_dir(&root)
            .status()
            .unwrap()
            .success()
    );
    fs::write(&source, bytes).unwrap();
    assert!(execution_backend(&root, Some(&declared)).is_err());
    assert!(
        std::process::Command::new("git")
            .args(["reset", "--quiet", "HEAD", "--", "src/lib.rs"])
            .current_dir(&root)
            .status()
            .unwrap()
            .success()
    );
    execution_backend(&root, Some(&declared)).unwrap();
    let original = fixture.backend.join("src/lib.rs");
    let bytes = fs::read(&original).unwrap();
    fs::write(&original, b"original drift").unwrap();
    assert!(request::execution_backend(&root, Some(&declared), &SystemSourceObserver).is_err());
    fs::write(original, bytes).unwrap();
    request::execution_backend(&root, Some(&declared), &SystemSourceObserver).unwrap();
}

#[test]
fn published_selection_rejects_mixed_roots_manifest_changes_and_observer_mismatch() {
    let fixture = Fixture::new();
    fixture.publication();
    let declared = selected(&fixture);
    let root = fixture.output.join("backend");
    assert!(execution_backend(&fixture.frontend, Some(&declared)).is_err());
    let mut mixed = declared.clone();
    mixed["fixture"] = json!({});
    let mut missing = declared.clone();
    missing.as_object_mut().unwrap().remove("execution_source");
    let mut wrong = declared.clone();
    wrong["execution_source"]["sha256"] = json!("0".repeat(64));
    let mut other = declared.clone();
    other["path"] = json!(fixture.frontend);
    for invalid in [mixed, missing, wrong, other] {
        assert!(execution_backend(&root, Some(&invalid)).is_err());
        assert!(request::execution_backend(&root, Some(&invalid), &SystemSourceObserver).is_err());
    }
    struct Wrong;
    impl SourceObserver for Wrong {
        fn snapshot(&self, _: &Path) -> Result<Value> {
            Ok(json!({}))
        }
    }
    assert!(request::execution_backend(&root, Some(&declared), &Wrong).is_err());
    let manifest = fixture.output.join("execution-source.json");
    let bytes = fs::read(&manifest).unwrap();
    fs::write(&manifest, b"{}").unwrap();
    assert!(execution_backend(&root, Some(&declared)).is_err());
    let mut changed: Value = serde_json::from_slice(&bytes).unwrap();
    changed["original"]["source_pair"]["sources"]["backend"]["head"] = json!("0".repeat(40));
    fs::write(&manifest, serde_json::to_vec(&changed).unwrap()).unwrap();
    assert!(execution_backend(&root, Some(&selected(&fixture))).is_err());
    fs::write(manifest, bytes).unwrap();
    execution_backend(&root, Some(&declared)).unwrap();
}

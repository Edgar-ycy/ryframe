use std::fs;

use super::{super as native, *};
use crate::clone_native_fixture::Fixture;

fn save(fixture: &Fixture) -> (PathBuf, Value) {
    document::write_new(&fixture.input, &fixture.value).unwrap();
    let wrapper = native::inspect_input(&fixture.root, &fixture.input)
        .unwrap()
        .wrapper;
    let plan = fixture.input.with_file_name("plan.json");
    document::write_new(&plan, &wrapper).unwrap();
    (plan, wrapper)
}

#[test]
fn held_plan_issues_same_result_and_protects_the_real_object_path() {
    let fixture = Fixture::new();
    let (plan, wrapper) = save(&fixture);
    let object = fixture.export.join("file.bin");
    let writer = fs::OpenOptions::new().write(true).open(&object).unwrap();
    assert!(HeldPlanObjects::open(&fixture.root, &plan, &wrapper).is_err());
    drop(writer);

    let window = HeldPlanObjects::open(&fixture.root, &plan, &wrapper).unwrap();
    assert_eq!(window.issue().unwrap(), wrapper);
    assert!(fs::write(&object, b"edit").is_err());
    assert!(fs::rename(&object, object.with_extension("old")).is_err());
    assert!(fs::rename(&fixture.export, fixture.export.with_extension("old")).is_err());
    window.check_end().unwrap();
    drop(window);

    fs::write(&object, b"edit").unwrap();
    assert!(native::issue(&fixture.root, &plan, &wrapper).is_err());
    assert!(HeldPlanObjects::open(&fixture.root, &plan, &wrapper).is_err());
}

#[test]
fn held_plan_rejects_different_descriptors_roots_and_object_declarations() {
    let fixture = Fixture::new();
    let (plan, wrapper) = save(&fixture);
    let window = HeldPlanObjects::open(&fixture.root, &plan, &wrapper).unwrap();
    let original = &fixture.value["objects"];
    let artifact = &original[4]["entries"][0]["artifact"];
    assert!(window.bound_object(&fixture.export, artifact).is_ok());
    assert!(window.bound_object(&fixture.root, artifact).is_err());

    for (field, value) in [
        ("file", json!("different.bin")),
        ("bytes", json!(5)),
        ("sha256", json!("0".repeat(64))),
    ] {
        let mut changed = artifact.clone();
        changed[field] = value;
        assert!(window.bound_object(&fixture.export, &changed).is_err());
    }

    let mut changed = original.clone();
    changed[4]["entries"][0]["metadata"] = json!({"different":true});
    assert!(
        window
            .require_declaration(&fixture.export, &changed)
            .is_err()
    );
    changed[4]["entries"] = json!([]);
    assert!(
        window
            .require_declaration(&fixture.export, &changed)
            .is_err()
    );

    let mut repeated = original.clone();
    let entries = repeated[4]["entries"].as_array_mut().unwrap();
    entries.push(original[4]["entries"][0].clone());
    assert_eq!(declared_artifacts(&repeated).unwrap().len(), 1);
    repeated[4]["entries"][1]["artifact"]["sha256"] = json!("0".repeat(64));
    assert!(declared_artifacts(&repeated).is_err());
}

#[test]
fn held_plan_still_verifies_sql_evidence_and_exact_input_bytes() {
    let fixture = Fixture::new();
    let (plan, wrapper) = save(&fixture);
    let window = HeldPlanObjects::open(&fixture.root, &plan, &wrapper).unwrap();
    for descriptor in [
        &fixture.value["databases"][0]["artifact"],
        &fixture.value["evidence"]["source_stopped"],
    ] {
        let file = fixture.export.join(descriptor["file"].as_str().unwrap());
        let original = fs::read(&file).unwrap();
        let mut changed = original.clone();
        changed.push(b' ');
        fs::write(&file, changed).unwrap();
        assert!(window.issue().is_err());
        fs::write(&file, original).unwrap();
        assert_eq!(window.issue().unwrap(), wrapper);
    }
    let declaration = fs::read(&fixture.input).unwrap();
    let mut changed = declaration.clone();
    changed.push(b' ');
    fs::write(&fixture.input, changed).unwrap();
    assert!(window.issue().is_err());
    fs::write(&fixture.input, declaration).unwrap();
    window.check_end().unwrap();

    let mut bytes = fs::read(&plan).unwrap();
    bytes.push(b' ');
    fs::write(&plan, bytes).unwrap();
    assert!(window.issue().is_err());
    assert!(window.check_end().is_err());
}

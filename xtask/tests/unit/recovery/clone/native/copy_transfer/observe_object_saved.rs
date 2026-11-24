use super::*;

fn saved_absence() -> (Fixture, Observed) {
    let fixture = Fixture::new();
    let fake = FakeS3::new([Step::Owner, head(absent()), Step::Owner, head(absent())]);
    let observed = observe(fixture.request(), &fake, &|| Ok(())).unwrap();
    fake.exhausted();
    assert_eq!(observed.receipt()["status"], "object_absent");
    observed
        .check_saved(fixture.temp.path(), &fixture.work)
        .unwrap();
    (fixture, observed)
}

fn assert_saved_rejected(fixture: &Fixture, observed: &Observed, reason: &str) {
    let receipt = fixture.directory.join("observation.json");
    let before = fs::read(&receipt).unwrap();
    let error = observed
        .check_saved(fixture.temp.path(), &fixture.work)
        .unwrap_err()
        .to_string();
    assert!(error.contains(reason), "{error}");
    assert_eq!(fs::read(receipt).unwrap(), before);
}

#[test]
fn saved_observation_rejects_changed_diagnostic_bytes() {
    let (fixture, observed) = saved_absence();
    let diagnostic = fixture.directory.join("head-before.diagnostic.json");
    let mut changed = fs::read(&diagnostic).unwrap();
    let original = serde_json::from_slice::<Value>(&changed).unwrap();
    *changed.iter_mut().find(|byte| **byte == b' ').unwrap() = b'\t';
    assert_eq!(serde_json::from_slice::<Value>(&changed).unwrap(), original);
    fs::write(&diagnostic, &changed).unwrap();

    assert_saved_rejected(
        &fixture,
        &observed,
        "目标对象原始证据缺失、改变或包含额外文件",
    );
    assert_eq!(fs::read(diagnostic).unwrap(), changed);
}

#[test]
fn saved_observation_rejects_evidence_outside_its_directory_after_rebinding() {
    let (fixture, mut observed) = saved_absence();
    let name = "head-before.diagnostic.json";
    let diagnostic = fixture.directory.join(name);
    let foreign = fixture.work.join(name);
    let original = fs::read(&diagnostic).unwrap();
    fs::write(&foreign, &original).unwrap();

    let evidence = observed.receipt["evidence"].as_object_mut().unwrap();
    let binding = evidence.remove(name).unwrap();
    evidence.insert(format!("../{name}"), binding);
    let path = fixture.directory.join("observation.json");
    fs::write(&path, serde_json::to_vec(&observed.receipt).unwrap()).unwrap();
    observed.descriptor = common::binding(&path).unwrap();

    assert_saved_rejected(
        &fixture,
        &observed,
        "目标对象原始证据缺失、改变或包含额外文件",
    );
    assert_eq!(fs::read(foreign).unwrap(), original);
    assert_eq!(fs::read(diagnostic).unwrap(), original);
}

#[test]
fn saved_observation_rejects_a_missing_required_evidence_file() {
    let (fixture, observed) = saved_absence();
    let diagnostic = fixture.directory.join("head-before.diagnostic.json");
    assert!(!observed.receipt()["evidence"]["head-before.diagnostic.json"].is_null());
    fs::remove_file(&diagnostic).unwrap();

    assert_saved_rejected(
        &fixture,
        &observed,
        "目标对象原始证据缺失、改变或包含额外文件",
    );
    assert!(!diagnostic.exists());
}

#[test]
fn saved_observation_rejects_an_omitted_evidence_manifest_even_with_bound_bytes() {
    let (fixture, mut observed) = saved_absence();
    observed.receipt["evidence"] = json!({});
    let path = fixture.directory.join("observation.json");
    fs::write(&path, serde_json::to_vec(&observed.receipt).unwrap()).unwrap();
    observed.descriptor = common::binding(&path).unwrap();

    assert_saved_rejected(
        &fixture,
        &observed,
        "目标对象原始证据缺失、改变或包含额外文件",
    );
    assert!(
        fixture
            .directory
            .join("head-before.diagnostic.json")
            .is_file()
    );
}

#[test]
fn saved_observation_rejects_a_hard_link_even_with_identical_diagnostic_bytes() {
    let (fixture, observed) = saved_absence();
    let diagnostic = fixture.directory.join("head-before.diagnostic.json");
    let alias = fixture.work.join("same-diagnostic.json");
    fs::hard_link(&diagnostic, &alias).unwrap();
    assert_eq!(fs::read(&diagnostic).unwrap(), fs::read(&alias).unwrap());

    assert_saved_rejected(&fixture, &observed, "存在额外硬链接");
    assert_eq!(fs::read(diagnostic).unwrap(), fs::read(alias).unwrap());
}

#[cfg(any(unix, windows))]
#[test]
fn saved_observation_rejects_a_link_to_identical_diagnostic_bytes() {
    let (fixture, observed) = saved_absence();
    let diagnostic = fixture.directory.join("head-before.diagnostic.json");
    let original = fixture.work.join("original-diagnostic.json");
    fs::rename(&diagnostic, &original).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&original, &diagnostic).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&original, &diagnostic).unwrap();
    let bytes = fs::read(&original).unwrap();
    assert_eq!(fs::read(&diagnostic).unwrap(), bytes);

    assert_saved_rejected(&fixture, &observed, "链接或 junction");
    assert_eq!(fs::read(original).unwrap(), bytes);
    assert!(
        fs::symlink_metadata(diagnostic)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn saved_observation_rejects_an_incomplete_failure_marker() {
    let (fixture, observed) = saved_absence();
    let marker = fixture.directory.join("failure.json");
    fs::write(&marker, b"{").unwrap();

    assert_saved_rejected(&fixture, &observed, "对象证据目录含失败标记");
    assert_eq!(fs::read(marker).unwrap(), b"{");
}

#[cfg(any(unix, windows))]
#[test]
fn saved_observation_rejects_a_dangling_failure_marker() {
    let (fixture, observed) = saved_absence();
    let marker = fixture.directory.join("failure.json");
    let missing = fixture.work.join("missing-failure.json");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&missing, &marker).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&missing, &marker).unwrap();
    assert!(!marker.exists());

    assert_saved_rejected(&fixture, &observed, "对象证据目录含失败标记");
    assert!(
        fs::symlink_metadata(marker)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(!missing.exists());
}

use super::*;
use crate::{
    recovery::clone::native::{
        json as documents,
        target_verify::{evidence_prepare, prepare_state, resume_baseline, tree},
    },
    target_verify_fixture::Fixture,
};
use std::{fs, path::PathBuf};

struct Case<'a> {
    fixture: &'a Fixture,
    target: PathBuf,
}

impl<'a> Case<'a> {
    fn new(fixture: &'a Fixture, label: &str) -> Self {
        let workspace = fixture.local.join(label);
        fs::create_dir(&workspace).unwrap();
        let target = workspace.join("target");
        fs::create_dir(&target).unwrap();
        let mut registration = documents::read(&fixture.local.join("registration.json")).unwrap();
        registration["target_directory"] = json!(target);
        documents::write_new(&workspace.join("registration.json"), &registration).unwrap();
        fs::copy(
            fixture.target.join("request.json"),
            target.join("request.json"),
        )
        .unwrap();
        Self { fixture, target }
    }

    fn copy(&self, name: &str) -> PathBuf {
        let label = name.rsplit_once('-').unwrap().0;
        let suffix = if name.ends_with(".command.json") {
            "command.json"
        } else {
            "json"
        };
        let output = format!("{label}-{}.{suffix}", uuid::Uuid::new_v4().simple());
        let path = self.target.join(output);
        fs::copy(self.fixture.target.join(name), &path).unwrap();
        path
    }

    fn intent(&self, id: &str) -> String {
        let before = fs::read_dir(&self.target)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                (
                    path.file_name().unwrap().to_str().unwrap().to_owned(),
                    common::binding(&path).unwrap(),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        let name = format!("resume-prepare-{id}.intent.json");
        documents::write_new(&self.target.join(&name),&json!({"format_version":1,"kind":"devex-clone-prepare-resume-intent",
            "attempt":id,"at":chrono::Utc::now().to_rfc3339(),"request":self.descriptor(),"files_before":before})).unwrap();
        name
    }

    fn descriptor(&self) -> Value {
        common::binding(&self.fixture.local.join("request.json")).unwrap()
    }

    fn publish_document(&self) {
        let mut value = documents::read(&self.fixture.target.join("prepare.json")).unwrap();
        value["prepared_at"] = json!(chrono::Utc::now().to_rfc3339());
        documents::write_new(&self.target.join("prepare.json"), &value).unwrap();
    }

    fn confirm(&self, id: &str, intent: &str) {
        documents::write_new(&self.target.join(format!("resume-prepare-{id}.confirmed.json")),&json!({
            "format_version":1,"kind":"devex-clone-prepare-resume-confirmed","attempt":id,"at":chrono::Utc::now().to_rfc3339(),
            "intent":common::binding(&self.target.join(intent)).unwrap(),"prepare":common::binding(&self.target.join("prepare.json")).unwrap(),
            "request":common::binding(&self.target.join("request.json")).unwrap(),"remote_writes":0})).unwrap();
    }

    fn validate(&self) -> crate::Result<()> {
        resume_baseline::validate(
            &self.fixture.backend,
            &self.target,
            &tree::snapshot(&self.fixture.backend, &self.target, false)?,
            &self.descriptor(),
            &self.fixture.request,
        )
    }
}

fn recording(fixture: &Fixture) -> Vec<String> {
    let files = documents::read(&fixture.local.join("prepared-files.json")).unwrap();
    let mut names = files["files"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| {
            let name = item["path"].as_str().unwrap();
            (!matches!(name, "request.json" | "prepare.json")).then(|| name.to_owned())
        })
        .collect::<BTreeSet<_>>();
    let candidate = Selected {
        value: fixture.request["storage"].clone(),
        runtime: None,
    };
    let mut ordered = Vec::new();
    for slot in slots(&fixture.backend, &fixture.request, &candidate).unwrap() {
        let name = names
            .iter()
            .find(|name| {
                category(name) == slot.name
                    && matches_slot(
                        &documents::read(&fixture.target.join(name)).unwrap(),
                        &slot.expected,
                        true,
                    )
                    .is_ok()
            })
            .unwrap()
            .clone();
        names.remove(&name);
        ordered.push(name);
    }
    assert_eq!(
        ordered.len(),
        31,
        "四库、五桶及五项两轮内核观察必须完整保留"
    );
    assert!(names.is_empty());
    ordered
}

#[test]
fn every_readonly_interruption_prefix_resumes_without_removing_or_rebinding_history() {
    let fixture = Fixture::for_status();
    let ordered = recording(&fixture);
    for length in 0..=ordered.len() {
        let case = Case::new(&fixture, &format!("prefix-{length}"));
        for name in ordered.iter().take(length) {
            case.copy(name);
        }
        let before = tree::snapshot(&fixture.backend, &case.target, false).unwrap();
        let first = "11111111111141118111111111111111";
        case.intent(first);
        // 模拟该 intent 发布后被强制中断；下一次只能显式接管其完整前像。
        let id = "22222222222242228222222222222222";
        let intent = case.intent(id);
        for name in &ordered {
            case.copy(name);
        }
        case.publish_document();
        case.confirm(id, &intent);
        case.validate().unwrap();
        let after = tree::snapshot(&fixture.backend, &case.target, false).unwrap();
        for entry in before.as_array().unwrap() {
            assert!(after.as_array().unwrap().contains(entry));
        }
        assert_eq!(
            evidence_prepare::records(&fixture.backend, &case.target, &case.descriptor()).unwrap()
                ["pending"],
            json!([])
        );
    }
}

#[test]
fn preserved_published_prepare_is_verified_again_without_rewriting_its_bytes() {
    let fixture = Fixture::for_status();
    let ordered = recording(&fixture);
    let case = Case::new(&fixture, "published-prepare");
    for name in &ordered {
        case.copy(name);
    }
    fs::copy(
        fixture.target.join("prepare.json"),
        case.target.join("prepare.json"),
    )
    .unwrap();
    let before = fs::read(case.target.join("prepare.json")).unwrap();
    let id = "33333333333343338333333333333333";
    let intent = case.intent(id);
    for name in &ordered {
        case.copy(name);
    }
    case.confirm(id, &intent);
    case.validate().unwrap();
    assert_eq!(fs::read(case.target.join("prepare.json")).unwrap(), before);
    let active = resume_baseline::active(
        &fixture.backend,
        &case.target,
        &tree::snapshot(&fixture.backend, &case.target, false).unwrap(),
        &fixture.request,
    )
    .unwrap();
    assert_eq!(active.as_array().unwrap().len(), 31);
}

#[test]
fn bound_history_tampering_unknown_files_and_unregistered_generation_fail_closed() {
    let fixture = Fixture::for_status();
    let case = Case::new(&fixture, "mutated-history");
    let ordered = recording(&fixture);
    case.copy(&ordered[0]);
    let id = "44444444444444448444444444444444";
    let intent = case.intent(id);
    for name in &ordered {
        case.copy(name);
    }
    case.publish_document();
    case.confirm(id, &intent);
    case.validate().unwrap();
    let duplicate = case.copy(&ordered[0]);
    assert!(case.validate().is_err(), "重复观察不能成为未登记的额外尝试");
    fs::remove_file(duplicate).unwrap();
    case.validate().unwrap();
    fs::write(case.target.join("unknown.bin"), b"unowned").unwrap();
    assert!(case.validate().is_err());
    fs::remove_file(case.target.join("unknown.bin")).unwrap();
    let old = documents::read(&case.target.join(&intent)).unwrap();
    let bound = old["files_before"]
        .as_object()
        .unwrap()
        .keys()
        .find(|name| name.starts_with("storage-layout-"))
        .unwrap();
    let path = case.target.join(bound);
    let bytes = fs::read(&path).unwrap();
    fs::write(&path, b"{}").unwrap();
    assert!(case.validate().is_err());
    fs::write(&path, bytes).unwrap();
    let path = case.target.join("prepare.json");
    let mut prepared = documents::read(&path).unwrap();
    prepared["generation"]["storage"]["redis"]["pid"] = json!(987654);
    fs::write(&path, serde_json::to_vec(&prepared).unwrap()).unwrap();
    // 同步终态 binding，确保拒绝来自账本代次校验，不能仅靠旧文件 SHA 不匹配假通过。
    let terminal_path = case
        .target
        .join(format!("resume-prepare-{id}.confirmed.json"));
    let mut terminal = documents::read(&terminal_path).unwrap();
    terminal["prepare"] = common::binding(&path).unwrap();
    fs::write(&terminal_path, serde_json::to_vec(&terminal).unwrap()).unwrap();
    assert!(case.validate().is_err());
}

#[test]
fn write_intents_cannot_be_adopted_as_readonly_prepare_even_with_valid_request() {
    let fixture = Fixture::for_status();
    for name in [
        "create-shared-control.intent.json",
        "sentinel.intent.json",
        "reset.intent.json",
    ] {
        let case = Case::new(
            &fixture,
            &format!("write-{}", uuid::Uuid::new_v4().simple()),
        );
        fs::write(case.target.join(name), b"{}").unwrap();
        let before = tree::snapshot(&fixture.backend, &case.target, false).unwrap();
        let state =
            prepare_state::state(&fixture.backend, &case.target, &case.descriptor()).unwrap();
        assert_eq!(state["resumable"], false);
        assert_eq!(
            tree::snapshot(&fixture.backend, &case.target, false).unwrap(),
            before
        );
    }
}

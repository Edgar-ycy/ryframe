use std::fs;

use crate::clone_native_fixture::{Fixture, snapshot};

use super::super::proofs::Proofs;
use super::*;

const COPY_STAGE: &str = "source_to_seed";

struct SavedPlan {
    _temp: tempfile::TempDir,
    backend: PathBuf,
    copy: PathBuf,
    manifest: Value,
    record: Value,
    declaration: Value,
    wrapper: Value,
}

impl SavedPlan {
    fn new(stage: &str) -> Self {
        let temp = tempfile::tempdir_in(crate::workspace::root_dir()).unwrap();
        let backend = crate::recovery::fixture_control::portable_path(
            &fs::canonicalize(temp.path()).unwrap(),
        );
        let local = backend.join(".local-tests");
        let copy = local.join("copy");
        fs::create_dir_all(&copy).unwrap();
        let environment = json!({"environment":{"APP_ENV":"test"}});
        let manifest = json!({
            "copy_directory":copy,
            "copy_stage":COPY_STAGE,
            "source_export":write_binding(&local.join("export.json"), &json!({"source":true})),
            "initialized":write_binding(&local.join("initialized.json"), &json!({"target":true})),
            "source_environment":write_binding(&local.join("source-environment.json"), &environment),
            "target_environment":write_binding(&local.join("target-environment.json"), &environment),
        });
        let declaration = json!({"format_version":1,"copy_id":"saved-plan","copy_stage":stage});
        let input = copy.join("input.json");
        document::write_new(&input, &declaration).unwrap();
        let binding = digest::file(&input).unwrap();
        let mut plan = json!({"format_version":1,"kind":"devex-clone-offline-plan",
            "copy_id":"saved-plan","copy_stage":stage,"input_sha256":digest::plan_hash(&declaration).unwrap(),
            "status":"offline_verified","execution_authorized":false,"target_ready":false});
        sign_plan(&mut plan);
        let facts = Facts {
            source_generation: &json!({"source":true}),
            target_generation: &json!({"target":true}),
            source_observation: &json!({"shared-control":{},"shared":{},"dedicated-a":{},"dedicated-b":{}}),
            declaration: &declaration,
        };
        let record = Files::bind(&backend, &manifest, &manifest["source_export"])
            .unwrap()
            .value(&facts, &plan)
            .unwrap();
        let wrapper = json!({"format_version":1,
            "input_path":digest::path_text(&input, "原计划输入").unwrap(),
            "input_file":{"bytes":binding.bytes,"sha256":binding.sha256},"plan":plan});
        document::write_new(&copy.join("plan.json"), &wrapper).unwrap();
        document::write_new(&copy.join("session.json"), &record).unwrap();
        Self {
            _temp: temp,
            backend,
            copy,
            manifest,
            record,
            declaration,
            wrapper,
        }
    }

    fn files(&self) -> Files<'_> {
        Files::bind(
            &self.backend,
            &self.manifest,
            &self.manifest["source_export"],
        )
        .unwrap()
    }

    fn manifest_stage(&self) -> &str {
        self.manifest["copy_stage"].as_str().unwrap()
    }

    fn snapshot(&self) -> BTreeMap<String, Vec<u8>> {
        fs::read_dir(&self.copy)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.file_name().into_string().unwrap(),
                    fs::read(entry.path()).unwrap(),
                )
            })
            .collect()
    }
}

fn write_binding(path: &Path, value: &Value) -> Value {
    document::write_new(path, value).unwrap();
    common::binding(path).unwrap()
}

fn sign_plan(plan: &mut Value) {
    plan.as_object_mut().unwrap().remove("plan_sha256");
    plan["plan_sha256"] = json!(digest::plan_hash(plan).unwrap());
}

#[test]
fn binds_generation_with_the_same_canonical_bytes_as_existing_copy_records() {
    let source = json!({"bytes":3,"path":"source.json","sha256":"a".repeat(64)});
    let target = json!({"bytes":5,"path":"target.json","sha256":"b".repeat(64)});
    let mut files = Files {
        backend: Path::new(""),
        root: PathBuf::new(),
        source_export: &source,
        initialized: &target,
        source_environment: BTreeMap::from([("APP_ENV".into(), "test".into())]),
        target_environment: BTreeMap::from([
            ("APP_ENV".into(), "test".into()),
            ("TMP".into(), "D:/临时".into()),
            ("TEMP".into(), "D:/临时".into()),
        ]),
        protected: Inputs::open([]).unwrap(),
    };
    let facts = Facts {
        source_generation: &json!({"source":"源"}),
        target_generation: &json!({"target":"目标"}),
        source_observation: &json!({"shared-control":{},"shared":{},"dedicated-a":{},"dedicated-b":{}}),
        declaration: &Value::Null,
    };
    let plan = json!({"plan_sha256":"c".repeat(64)});
    let record = files.value(&facts, &plan).unwrap();
    assert_eq!(
        record["generation_sha256"],
        "ea76cd3e39ae92c6fc03c01dc14aaae21c6d44c78073a7a4a9412f52a6962a98"
    );
    assert_eq!(record["source_observation"], *facts.source_observation);
    assert_eq!(record["clone_verified"], false);
    assert_eq!(record["restore_qualified"], false);
    files
        .target_environment
        .insert("TEMP".into(), "D:/漂移".into());
    assert_ne!(
        files.value(&facts, &plan).unwrap()["generation_sha256"],
        record["generation_sha256"]
    );
}

#[test]
fn saved_copy_preflight_holds_the_original_three_files_without_writes() {
    let fixture = SavedPlan::new(COPY_STAGE);
    let before = fixture.snapshot();
    assert_eq!(
        before.keys().map(String::as_str).collect::<Vec<_>>(),
        ["input.json", "plan.json", "session.json"]
    );
    let files = fixture.files();
    let continuation = files.continuation(fixture.manifest_stage()).unwrap();
    assert_eq!(continuation.record, fixture.record);
    assert_eq!(continuation.declaration, fixture.declaration);
    continuation.check().unwrap();
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn prepares_once_and_hydrates_against_original_copy_proofs() {
    let fixture = Fixture::new();
    let local = fixture.root.join(".local-tests");
    let copy = local.join("copy");
    let target = local.join("target");
    fs::create_dir(&copy).unwrap();
    fs::create_dir(&target).unwrap();

    let (manifest, initialized) = initial_manifest(&fixture, &copy, &target);
    let initial = &manifest["initialized"];
    let export = &manifest["source_export"];
    let source_stopped = &fixture.value["evidence"]["source_stopped"];
    let original_receipts = [
        fixture.export.join("export.json"),
        target.join("initialized.json"),
    ];
    let read_receipts = || {
        original_receipts
            .each_ref()
            .map(|path| fs::read(path).unwrap())
    };
    let originals = read_receipts();
    let proofs = publish_initial_proofs(&fixture, source_stopped, initial, &initialized);
    let mut declaration = fixture.value.clone();
    declaration["evidence"] = proofs.descriptors.clone();
    let source_generation = json!({"stopped":true});
    let target_generation = json!({"initialized":true});
    let source_observation =
        json!({"shared-control":{},"shared":{},"dedicated-a":{},"dedicated-b":{}});
    let facts = || Facts {
        source_generation: &source_generation,
        target_generation: &target_generation,
        source_observation: &source_observation,
        declaration: &declaration,
    };
    let bind = || Files::bind(&fixture.root, &manifest, export).unwrap();
    let prepared = bind().prepare(facts()).unwrap();
    prepared.check().unwrap();
    proofs.checkpoint().unwrap();
    let (original_plan, original_record) = (prepared.plan.clone(), prepared.record.clone());
    assert_eq!(original_plan["copy_stage"], COPY_STAGE);
    assert_eq!(&original_record["source_export"], export);
    assert_eq!(&original_record["initialized"], initial);
    drop((prepared, proofs));
    assert_eq!(read_receipts(), originals);
    assert_eq!(snapshot(&copy).len(), 3);
    let before = snapshot(&fixture.root);
    let proof_directories = || {
        fs::read_dir(&fixture.export)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| {
                name.to_str()
                    .is_some_and(|name| name.starts_with("target-proof-"))
            })
            .collect::<BTreeSet<_>>()
    };
    let original_proof_directories = proof_directories();
    assert_eq!(original_proof_directories.len(), 1);

    for _ in 0..2 {
        let files = bind();
        let continuation = files.continuation(COPY_STAGE).unwrap();
        let proofs = Proofs::open(
            &fixture.export,
            &continuation.declaration["evidence"],
            source_stopped,
            initial,
            &initialized,
        )
        .unwrap();
        let hydrated = files.hydrate(&continuation, facts()).unwrap();
        assert_eq!(proofs.descriptors, declaration["evidence"]);
        assert_eq!(hydrated.plan, original_plan);
        assert_eq!(hydrated.record, original_record);
        proofs.checkpoint().unwrap();
        hydrated.check().unwrap();
        assert_eq!(snapshot(&fixture.root), before);
        assert_eq!(proof_directories(), original_proof_directories);
    }
}

fn publish_initial_proofs(
    fixture: &Fixture,
    source_stopped: &Value,
    initial: &Value,
    initialized: &Value,
) -> Proofs {
    let stopped = json!({
        "status":"fresh_target_reverified","initialized":initial,
        "inventory":initialized["inventory"],"remote_writes":0,
        "target_ready":false,"clone_verified":false,"restore_qualified":false,
    });
    Proofs::publish(
        &fixture.root,
        &fixture.export,
        source_stopped,
        initial,
        initialized,
        &stopped,
    )
    .unwrap()
}

fn initial_manifest(fixture: &Fixture, copy: &Path, target: &Path) -> (Value, Value) {
    let local = copy.parent().unwrap();
    let export = write_binding(
        &fixture.export.join("export.json"),
        &json!({
            "artifact_root":fixture.export,"evidence":{"generation-after":fixture.value["evidence"]["source_stopped"]},
            "databases":fixture.value["databases"],"objects":fixture.value["objects"],
        }),
    );
    let observations = json!({"target-db":{"kind":"database","tables":{},"preserved":{}}});
    let initialized = json!({"inventory":{"observations":observations}});
    let initial = write_binding(&target.join("initialized.json"), &initialized);
    let environment = json!({"environment":{"APP_ENV":"test"}});
    let manifest = json!({
        "copy_directory":copy,"copy_stage":COPY_STAGE,"source_export":export,"initialized":initial,
        "source_environment":write_binding(&local.join("source-environment.json"), &environment),
        "target_environment":write_binding(&local.join("target-environment.json"), &environment),
    });
    (manifest, initialized)
}

#[derive(Clone, Copy, Debug)]
enum Mismatch {
    InputPath,
    InputBytes,
    PlanProjection,
    SessionDigest,
    InputSemanticDigest,
    Stage,
}

#[test]
fn saved_copy_preflight_rejects_exact_binding_mismatches_without_writes() {
    for mismatch in [
        Mismatch::InputPath,
        Mismatch::InputBytes,
        Mismatch::PlanProjection,
        Mismatch::SessionDigest,
        Mismatch::InputSemanticDigest,
        Mismatch::Stage,
    ] {
        let stage = if matches!(mismatch, Mismatch::Stage) {
            "seed_to_arm"
        } else {
            COPY_STAGE
        };
        let fixture = SavedPlan::new(stage);
        let input = fixture.copy.join("input.json");
        let mut wrapper = fixture.wrapper.clone();
        let mut record = fixture.record.clone();
        let expected = match mismatch {
            Mismatch::InputPath => {
                let alias = fixture.copy.with_file_name("identical-input.json");
                fs::copy(&input, &alias).unwrap();
                assert_eq!(fs::read(&alias).unwrap(), fs::read(&input).unwrap());
                wrapper["input_path"] = json!(digest::path_text(&alias, "同字节异路径").unwrap());
                "原离线计划没有绑定固定复制目录中的输入原字节"
            }
            Mismatch::InputBytes => {
                let mut bytes = fs::read(&input).unwrap();
                bytes.extend_from_slice(b"\n ");
                fs::write(&input, bytes).unwrap();
                assert_eq!(document::read(&input).unwrap(), fixture.declaration);
                "原离线计划没有绑定固定复制目录中的输入原字节"
            }
            Mismatch::PlanProjection => {
                wrapper["plan"]["copy_id"] = json!("changed-projection");
                "原计划、固定输入或复制阶段与复制会话绑定不同"
            }
            Mismatch::SessionDigest => {
                record["plan_sha256"] = json!("0".repeat(64));
                "原计划、固定输入或复制阶段与复制会话绑定不同"
            }
            Mismatch::InputSemanticDigest => {
                wrapper["plan"]["input_sha256"] = json!("0".repeat(64));
                sign_plan(&mut wrapper["plan"]);
                record["plan_sha256"] = wrapper["plan"]["plan_sha256"].clone();
                "原计划、固定输入或复制阶段与复制会话绑定不同"
            }
            Mismatch::Stage => "原计划、固定输入或复制阶段与复制会话绑定不同",
        };
        if wrapper != fixture.wrapper {
            document::write_replace(&fixture.copy.join("plan.json"), &wrapper).unwrap();
        }
        if record != fixture.record {
            document::write_replace(&fixture.copy.join("session.json"), &record).unwrap();
        }
        let before = fixture.snapshot();
        let files = fixture.files();
        let error = files
            .continuation(fixture.manifest_stage())
            .err()
            .expect("绑定失配必须在前置核对拒绝");
        assert_eq!(error.to_string(), expected, "{mismatch:?}");
        assert_eq!(fixture.snapshot(), before, "{mismatch:?}");
    }
}

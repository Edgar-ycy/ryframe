use super::{
    evidence,
    producer::{self, Registration},
};
use crate::recovery::clone::native::{
    digest,
    lifecycle::{identity, state},
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

struct Fixture {
    run: PathBuf,
    args: Vec<String>,
    grant: Value,
}
impl Fixture {
    fn new() -> Self {
        let backend = crate::workspace::root_dir();
        let run = backend
            .join(".local-tests")
            .join(format!("identity-producer-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(run.join("seed-runtime/attempt-0002")).unwrap();
        fs::create_dir(run.join("run.lock")).unwrap();
        evidence::new(&run.join("manifest.json"), &json!({"kind":"isolated"})).unwrap();
        evidence::new(&run.join("plan.json"), &json!({"no_secrets":true})).unwrap();
        let executable = std::env::current_exe().unwrap();
        let tool = json!({"path":executable,"sha256":digest::file(&executable).unwrap().sha256});
        let owner = json!({"format_version":1,"directory":run,"manifest_sha256":producer::binding(&run.join("manifest.json")).unwrap()["sha256"],"identity":identity::current().unwrap()});

        evidence::new(&run.join("run.lock/owner.json"), &owner).unwrap();
        let manifest = producer::binding(&run.join("manifest.json")).unwrap();
        evidence::new(&run.join("seed-runtime.json"), &json!({
            "format_version":1,"kind":"devex-clone-seed-runtime","run_manifest":manifest,
            "post_copy":{},"post_verify":{},"node":{},"api_process":{},
            "identity_tool":tool,"identity_plan":producer::binding(&run.join("plan.json")).unwrap(),
            "identity_state":run.join("seed-runtime/identities")
        })).unwrap();
        state::initialize(&run).unwrap();
        let (registered, _) = state::begin(&run, "seed-runtime", "register", &json!({})).unwrap();
        state::finish(
            &run,
            registered,
            Some(&json!({
                "status":"seed_runtime_registered",
                "registration":producer::binding(&run.join("seed-runtime.json")).unwrap()
            })),
            None,
        )
        .unwrap();
        let (number, started) =
            state::begin(&run, "seed-runtime", "identities-apply", &json!({})).unwrap();
        assert_eq!(number, 2);
        state::bind_controller(&run, number, &owner, &started).unwrap();
        let args = [
            ("--run-dir", run.display().to_string()),
            ("--attempt", "2".into()),
            (
                "--identity-plan",
                run.join("plan.json").display().to_string(),
            ),
            (
                "--identity-state",
                run.join("seed-runtime/identities").display().to_string(),
            ),
            ("--identity-mode", "apply".into()),
            (
                "--identity-plan-sha256",
                digest::file(&run.join("plan.json")).unwrap().sha256,
            ),
        ]
        .into_iter()
        .flat_map(|(key, value)| [key.to_owned(), value])
        .collect::<Vec<_>>();
        let command = [
            vec![
                executable.display().to_string(),
                "__identity-producer".into(),
            ],
            args.clone(),
        ]
        .concat();
        let launch = run.join("seed-runtime/attempt-0002/session-launch.json");
        evidence::new(&launch,&json!({"format_version":1,"kind":"devex-post-producer-launch","run_manifest":producer::binding(&run.join("manifest.json")).unwrap(),"registration":producer::binding(&run.join("seed-runtime.json")).unwrap(),"controller":producer::binding(&run.join("controller-0002.json")).unwrap(),"attempt":2,"producer_kind":"identity-apply","command":command,"tool":tool,"source":producer::binding(&backend.join("xtask/src/data/performance_identities/producer.rs")).unwrap()})).unwrap();
        let token = uuid::Uuid::new_v4().to_string();
        let process = run.join("seed-runtime/attempt-0002/session-process.json");
        evidence::new(&process,&json!({"format_version":1,"kind":"devex-post-producer","identity":identity::current().unwrap(),"launch":producer::binding(&launch).unwrap(),"identity_lock_owner":token})).unwrap();
        let grant = json!({"operation":"start","run_dir":run,"attempt":2,"identity_lock_owner":token,"launch":producer::binding(&launch).unwrap(),"process":producer::binding(&process).unwrap()});
        Self { run, args, grant }
    }
    fn registration(&self) -> Registration {
        Registration::parse(&self.args, &crate::workspace::root_dir()).unwrap()
    }
    fn root(&self) -> PathBuf {
        crate::workspace::root_dir()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.run).unwrap();
    }
}

#[test]
fn native_producer_authority_binds_current_kernel_owner_launch_and_raw_plan() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture
            .registration()
            .authorize(&fixture.root(), &fixture.grant)
            .unwrap(),
        fixture.grant["identity_lock_owner"]
    );
    for (key, value) in [
        ("attempt", json!(3)),
        ("run_dir", json!(fixture.run.join("other"))),
        (
            "identity_lock_owner",
            json!(uuid::Uuid::new_v4().to_string()),
        ),
        ("process", json!({})),
        ("launch", json!({})),
    ] {
        let mut invalid = fixture.grant.clone();
        invalid[key] = value;
        assert!(
            fixture
                .registration()
                .authorize(&fixture.root(), &invalid)
                .is_err()
        );
    }
    fs::write(fixture.run.join("plan.json"), "changed").unwrap();
    assert!(
        fixture
            .registration()
            .authorize(&fixture.root(), &fixture.grant)
            .is_err()
    );
    assert!(!fixture.run.join("seed-runtime/identities").exists());
}
#[test]
fn native_producer_rejects_registration_tool_source_and_controller_drift() {
    for filename in [
        "seed-runtime.json",
        "controller-0002.json",
        "run.lock/owner.json",
        "state.json",
    ] {
        let fixture = Fixture::new();
        fs::write(fixture.run.join(filename), "{}").unwrap();
        assert!(
            fixture
                .registration()
                .authorize(&fixture.root(), &fixture.grant)
                .is_err()
        );
    }
}

#[test]
fn native_producer_requires_passed_registration_and_exact_published_receipt() {
    for case in ["failed", "wrong-result", "wrong-run"] {
        let fixture = Fixture::new();
        let mut saved =
            evidence::parse(&fs::read(fixture.run.join("state.json")).unwrap()).unwrap();
        match case {
            "failed" => saved["attempts"][0]["status"] = json!("failed"),
            "wrong-result" => {
                let path = fixture.run.join("results/0001.json");
                crate::recovery::clone::native::json::write_replace(
                    &path,
                    &json!({"status":"Private-Secret-Result","registration":{}}),
                )
                .unwrap();
                saved["attempts"][0]["result"] = producer::binding(&path).unwrap();
            }
            "wrong-run" => saved["manifest"] = json!({}),
            _ => unreachable!(),
        }
        crate::recovery::clone::native::json::write_replace(
            &fixture.run.join("state.json"),
            &saved,
        )
        .unwrap();
        let failure = fixture
            .registration()
            .authorize(&fixture.root(), &fixture.grant)
            .unwrap_err()
            .to_string();
        assert!(!failure.contains("Private-Secret-Result"));
        assert!(!fixture.run.join("seed-runtime/identities").exists());
    }
}
#[test]
fn native_producer_arguments_reject_ambiguous_or_outside_state_before_authorization() {
    let fixture = Fixture::new();
    for (key, value) in [
        ("--attempt", "0"),
        ("--attempt", "01"),
        ("--attempt", "9007199254740992"),
        ("--identity-mode", "plan"),
        ("--identity-state", "relative"),
        ("--identity-plan-sha256", "wrong"),
    ] {
        let mut arguments = fixture.args.clone();
        let index = arguments.iter().position(|item| item == key).unwrap() + 1;
        arguments[index] = value.into();
        assert!(Registration::parse(&arguments, &fixture.root()).is_err());
    }
    assert!(Registration::parse(&fixture.args[..10], &fixture.root()).is_err());
    let mut duplicate = fixture.args.clone();
    duplicate[2] = "--run-dir".into();
    assert!(Registration::parse(&duplicate, Path::new(&fixture.root())).is_err());
}

#[test]
fn granted_producer_rechecks_exact_plan_bytes_before_loading_any_configuration() {
    let fixture = Fixture::new();
    let state = fixture.run.join("seed-runtime/identities");
    let failure = super::execute_at(
        &fixture.run.join("plan.json"),
        &state,
        "apply",
        &uuid::Uuid::new_v4().to_string(),
        Some(&"a".repeat(64)),
        &mut |_| Ok(()),
    )
    .unwrap_err()
    .to_string();
    assert!(failure.contains("原始字节在授权后变化"));
    assert!(!state.exists());
}

#[test]
fn stopped_seed_guard_prevents_loading_private_configuration_or_creating_any_ledger() {
    let fixture = Fixture::new();
    let target = fixture.run.join("seed-runtime/identities");
    let mut phases = Vec::new();
    let error = super::execute_at(
        &fixture.run.join("missing-private-plan.json"),
        &target,
        "apply",
        &uuid::Uuid::new_v4().to_string(),
        None,
        &mut |phase| {
            phases.push(phase.to_owned());
            Err("seed 已失效".into())
        },
    )
    .unwrap_err()
    .to_string();
    assert_eq!(phases, vec!["before-load"]);
    assert!(error.contains("seed 已失效"));
    assert!(!target.exists());
}

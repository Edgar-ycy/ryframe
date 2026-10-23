use super::{
    evidence,
    producer::{self, Registration},
};
use crate::recovery::clone::native::{digest, lifecycle::identity};
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
        fs::create_dir_all(run.join("seed-runtime/attempt-0001")).unwrap();
        fs::create_dir(run.join("run.lock")).unwrap();
        evidence::new(&run.join("manifest.json"), &json!({"kind":"isolated"})).unwrap();
        evidence::new(&run.join("plan.json"), &json!({"no_secrets":true})).unwrap();
        let executable = std::env::current_exe().unwrap();
        let tool = json!({"path":executable,"sha256":digest::file(&executable).unwrap().sha256});
        let attempt =
            json!({"number":1,"stage":"seed-runtime","mode":"identities-apply","status":"running"});
        let owner = json!({"format_version":1,"directory":run,"manifest_sha256":producer::binding(&run.join("manifest.json")).unwrap()["sha256"],"identity":identity::current().unwrap()});
        evidence::new(&run.join("state.json"), &json!({"attempts":[attempt]})).unwrap();
        evidence::new(&run.join("run.lock/owner.json"), &owner).unwrap();
        evidence::new(&run.join("controller-0001.json"),&json!({"format_version":1,"kind":"devex-stage-controller","owner":owner,"attempt":1,"attempt_sha256":digest::plan_hash(&attempt).unwrap()})).unwrap();
        evidence::new(&run.join("seed-runtime.json"),&json!({"identity_tool":tool,"identity_plan":producer::binding(&run.join("plan.json")).unwrap(),"identity_state":run.join("seed-runtime/identities")})).unwrap();
        let args = [
            ("--run-dir", run.display().to_string()),
            ("--attempt", "1".into()),
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
        let launch = run.join("seed-runtime/attempt-0001/session-launch.json");
        evidence::new(&launch,&json!({"format_version":1,"kind":"devex-post-producer-launch","run_manifest":producer::binding(&run.join("manifest.json")).unwrap(),"registration":producer::binding(&run.join("seed-runtime.json")).unwrap(),"controller":producer::binding(&run.join("controller-0001.json")).unwrap(),"attempt":1,"producer_kind":"identity-apply","command":command,"tool":tool,"source":producer::binding(&backend.join("xtask/src/data/performance_identities/producer.rs")).unwrap()})).unwrap();
        let token = uuid::Uuid::new_v4().to_string();
        let process = run.join("seed-runtime/attempt-0001/session-process.json");
        evidence::new(&process,&json!({"format_version":1,"kind":"devex-post-producer","identity":identity::current().unwrap(),"launch":producer::binding(&launch).unwrap(),"identity_lock_owner":token})).unwrap();
        let grant = json!({"operation":"start","run_dir":run,"attempt":1,"identity_lock_owner":token,"launch":producer::binding(&launch).unwrap(),"process":producer::binding(&process).unwrap()});
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
        ("attempt", json!(2)),
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
        "controller-0001.json",
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
    )
    .unwrap_err()
    .to_string();
    assert!(failure.contains("原始字节在授权后变化"));
    assert!(!state.exists());
}

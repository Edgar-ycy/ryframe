use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use serde_json::{Value, json};

use super::{
    cli::{Command, DataCommand, PerformanceIdentitiesCommand, parse},
    data::performance_identities::{
        plan as native_plan, private_command, private_invocation_at, state as native_state,
    },
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    workspace::root_dir,
};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    directory: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = root_dir().join(".local-tests").join(format!(
            "xtask-performance-identities-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        Self { directory }
    }

    fn file(&self, name: &str) -> PathBuf {
        let target = self.directory.join(name);
        fs::write(&target, b"{}\n").unwrap();
        target
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.directory.exists() {
            fs::remove_dir_all(&self.directory).unwrap();
        }
    }
}

fn parse_command(arguments: &[String]) -> Result<Command, super::cli::CliError> {
    parse(arguments.to_vec()).map(|cli| cli.command)
}

fn arguments(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

#[test]
fn parses_three_explicit_write_operations_with_absolute_local_paths() {
    let fixture = Fixture::new();
    let environment = fixture.file("性能 environment.json");
    let output = fixture.directory.join("identity plan.json");
    let plan = fixture.file("existing plan.json");
    let state_dir = fixture.directory.join("identity state");
    let command = parse_command(&arguments(&[
        "data",
        "performance-identities",
        "plan",
        "--environment",
        environment.to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
        "--write",
    ]))
    .unwrap();
    assert_eq!(
        command,
        Command::Data(DataCommand::PerformanceIdentities(
            PerformanceIdentitiesCommand::Plan {
                environment,
                output,
            }
        ))
    );
    for operation in ["apply", "verify"] {
        let command = parse_command(&arguments(&[
            "data",
            "performance-identities",
            operation,
            "--plan",
            plan.to_str().unwrap(),
            "--state-dir",
            state_dir.to_str().unwrap(),
            "--write",
        ]))
        .unwrap();
        assert!(matches!(
            command,
            Command::Data(DataCommand::PerformanceIdentities(
                PerformanceIdentitiesCommand::Apply { .. }
                    | PerformanceIdentitiesCommand::Verify { .. }
            ))
        ));
    }
}

#[test]
fn rejects_ambiguous_options_and_every_non_local_path_before_dispatch() {
    let fixture = Fixture::new();
    let environment = fixture.file("environment.json");
    let output = fixture.directory.join("plan.json");
    let outside = root_dir().join("README.md");
    let parent_jump = fixture.directory.join("child/../plan.json");
    let existing_output = fixture.file("existing-output.json");
    let file_parent = fixture.file("file-parent").join("plan.json");
    let valid = [
        "data".to_owned(),
        "performance-identities".to_owned(),
        "plan".to_owned(),
        "--environment".to_owned(),
        environment.to_string_lossy().into_owned(),
        "--output".to_owned(),
        output.to_string_lossy().into_owned(),
        "--write".to_owned(),
    ];
    let mut cases = vec![valid[..valid.len() - 1].to_vec()];
    cases.push([valid.as_slice(), &["--write".to_owned()]].concat());
    cases.push([valid.as_slice(), &["--unknown".to_owned()]].concat());
    for invalid_output in [
        Path::new("relative.json"),
        outside.as_path(),
        parent_jump.as_path(),
        existing_output.as_path(),
        file_parent.as_path(),
    ] {
        let mut case = valid.clone();
        case[6] = invalid_output.to_string_lossy().into_owned();
        cases.push(case.to_vec());
    }
    let mut cross_operation = valid;
    cross_operation[2] = "apply".to_owned();
    cases.push(cross_operation.to_vec());
    for case in cases {
        assert!(parse_command(&case).is_err(), "参数应失败：{case:?}");
    }
}

#[test]
fn private_invocation_has_exact_json_without_a_script_or_public_arguments() {
    let fixture = Fixture::new();
    let environment = fixture.file("环境 清单.json");
    let output = fixture.directory.join("身份 计划.json");
    let command = PerformanceIdentitiesCommand::Plan {
        environment: environment.clone(),
        output: output.clone(),
    };
    let invocation = private_invocation_at(&command, &root_dir()).unwrap();
    assert!(!invocation.protocol.contains("tools/js"));
    assert!(!invocation.protocol.contains(['\n', '\r', '\0']));
    let protocol: Value = serde_json::from_str(&invocation.protocol).unwrap();
    assert_eq!(
        protocol.as_object().unwrap().keys().collect::<Vec<_>>(),
        [
            "environment",
            "format_version",
            "operation",
            "output",
            "write"
        ]
    );
    assert_eq!(protocol["format_version"], 1);
    assert_eq!(protocol["operation"], "plan");
    assert_eq!(protocol["environment"], environment.to_str().unwrap());
    assert_eq!(protocol["output"], output.to_str().unwrap());
    assert_eq!(protocol["write"], true);
}

#[test]
fn rejects_link_or_junction_components_even_when_the_target_exists() {
    let fixture = Fixture::new();
    let linked = fixture.directory.join("linked-scripts");
    create_directory_link(&root_dir().join("scripts"), &linked);
    let target = linked.join("private.json");
    let error = validate_local_test_path(&target, &root_dir(), LocalTestPathKind::ExistingFile)
        .unwrap_err();
    assert!(
        error.contains("链接") || error.contains("junction"),
        "{error}"
    );
    #[cfg(unix)]
    fs::remove_file(&linked).unwrap();
    #[cfg(windows)]
    fs::remove_dir(&linked).unwrap();
}

#[cfg(unix)]
fn create_directory_link(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

fn environment_fixture(fixture: &Fixture) -> Value {
    use super::recovery::clone::native::digest;
    let root = root_dir();
    let document = fixture.file("环境说明.md");
    fs::write(&document, "独立性能验收环境").unwrap();
    let runtime = fixture.directory.join("runtime");
    fs::create_dir(&runtime).unwrap();
    for filename in ["runtime.json", "api.json"] {
        fs::write(runtime.join(filename), b"{}").unwrap();
    }
    let connection = |database| json!({"host":"127.0.0.1","port":3306,"database":database,"username":"fixture","password":"Fixture-Database-Password!123","tls_mode":"required"});
    let admin = |tenant: &str, index| json!({"tenant_id":tenant,"username":"admin","password":"Fixture-Admin-Password!123","client_address":format!("198.18.30.{}",index+1)});
    json!({"format_version":1,"plan_id":"fixture","scope_id":"identity-fixture","backend_dir":root,"frontend_dir":root.parent().unwrap().join("ryframe-vue3"),
        "api_url":"http://127.0.0.1:18999/","frontend_url":"http://127.0.0.1:4199/",
        "environment_document":{"path":document,"sha256":digest::file(&document).unwrap().sha256},
        "runtime":{"directory":runtime,"receipt_sha256":digest::file(&runtime.join("runtime.json")).unwrap().sha256,"api_process_sha256":digest::file(&runtime.join("api.json")).unwrap().sha256},
        "database":{"python":root.join("python-fixture"),"mysql_client":root.join("mysql-fixture"),"server_uuid":"11111111-1111-1111-1111-111111111111","control":connection("identity_control"),"targets":[{"key":"shared","mode":"shared","connection":connection("identity_shared")}]},
        "system_admin":admin("system",0),"tenants":(1..=10).map(|index| { let tenant=format!("fixture-{index}"); json!({"slot":format!("tenant-{index:02}"),"tenant_id":tenant,"target_key":"shared","admin":admin(&tenant,index)}) }).collect::<Vec<_>>(),
        "passwords":{"system":"Fixture-User-Password!123","tenant":"Fixture-User-Password!123"},
        "quota":{"system_max_users":1000,"tenant_max_users":1000,"import_headroom_per_tenant":100},
        "pacing":{"contract":{"authority_sha256":"a".repeat(64),"request_interval_ms":1000,"operation_intervals_ms":{"post_auth_login":12250},"sample_prepare_wait_ms":60250},"bindings":{"login_budget_state":fixture.directory.join("login-budget.json")}}})
}

fn create_plan(environment: &Value, fixture: &Fixture) -> super::Result<Value> {
    let path = fixture.directory.join("private.json");
    fs::write(&path, serde_json::to_vec(environment)?)?;
    native_plan::create(environment, &root_dir(), &path)
}

#[test]
fn native_plan_is_offline_deterministic_and_keeps_two_hundred_fixed_identities() {
    let fixture = Fixture::new();
    let environment = environment_fixture(&fixture);
    let plan = create_plan(&environment, &fixture).unwrap();
    assert_eq!(plan, create_plan(&environment, &fixture).unwrap());
    let groups = plan["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 11);
    assert_eq!(groups[0]["users"].as_array().unwrap().len(), 100);
    assert_eq!(
        groups
            .iter()
            .skip(1)
            .filter(|group| group["users"].as_array().unwrap().len() == 10)
            .count(),
        10
    );
    let addresses = groups
        .iter()
        .flat_map(|group| group["users"].as_array().unwrap())
        .map(|user| user["client_address"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(addresses.len(), 200);
    assert_eq!(groups[0]["users"][0]["username"], "dv-fixture-001");
    assert_eq!(groups[10]["users"][9]["client_address"], "198.18.50.10");
    assert_eq!(
        groups[0]["permissions"],
        json!([
            "monitor:schedule:add",
            "monitor:schedule:run",
            "system:message:publish",
            "system:post:export"
        ])
    );
    assert_eq!(fs::read_dir(&fixture.directory).unwrap().count(), 3);
    let mut changed = plan.clone();
    changed["groups"][0]["count"] = json!(99);
    assert!(native_plan::validate(&changed).is_err());
}

#[test]
fn native_plan_rejects_invalid_tenant_targets_permissions_and_source_drift() {
    let fixture = Fixture::new();
    let environment = environment_fixture(&fixture);
    for (pointer, replacement) in [
        ("/tenants/1/tenant_id", json!("fixture-1")),
        ("/tenants/1/admin/client_address", json!("198.18.30.1")),
        ("/quota/system_max_users", json!(100)),
        ("/passwords/system", json!("")),
        ("/api_url", json!("https://example.test/")),
        ("/api_url", json!("http://private:secret@127.0.0.1/")),
        ("/database/control/database", json!("unsafe;drop")),
        ("/database/targets/0/connection/password", json!("")),
        ("/database/targets/0/mode", json!("dedicated")),
        (
            "/pacing/contract/operation_intervals_ms/post_auth_login",
            json!(0),
        ),
        ("/pacing/bindings/login_budget_state", json!("relative")),
        (
            "/pacing/bindings",
            json!({"login_budget_state":environment["pacing"]["bindings"]["login_budget_state"],
                "python":fixture.directory.join("unused-python.exe")}),
        ),
        ("/system_admin/client_address", json!("198.18.40.1")),
    ] {
        let mut changed = environment.clone();
        *changed.pointer_mut(pointer).unwrap() = replacement;
        assert!(create_plan(&changed, &fixture).is_err(), "{pointer}");
    }
    fs::write(
        environment["environment_document"]["path"]
            .as_str()
            .unwrap(),
        "已变化",
    )
    .unwrap();
    assert!(
        create_plan(&environment, &fixture)
            .unwrap_err()
            .to_string()
            .contains("SHA-256")
    );
}

#[test]
fn native_plan_new_file_publication_refuses_overwrite_and_duplicate_json_keys() {
    let fixture = Fixture::new();
    let environment = environment_fixture(&fixture);
    let input = fixture.file("input.json");
    fs::write(&input, serde_json::to_vec(&environment).unwrap()).unwrap();
    let output = fixture.directory.join("计划.json");
    assert_eq!(
        native_plan::write(&input, &output, &root_dir()).unwrap()["users"],
        200
    );
    let before = fs::read(&output).unwrap();
    assert!(native_plan::write(&input, &output, &root_dir()).is_err());
    assert_eq!(fs::read(&output).unwrap(), before);
    fs::write(&input, b"{\"format_version\":1,\"format_version\":2}").unwrap();
    assert!(native_plan::write(&input, &fixture.directory.join("bad.json"), &root_dir()).is_err());
    assert!(!fixture.directory.join("bad.json").exists());
}

#[test]
fn native_private_protocol_rejects_duplicates_invalid_encoding_and_over_limit() {
    let fixture = Fixture::new();
    let input = fixture.file("环境.json");
    let output = fixture.directory.join("计划.json");
    let request = json!({"format_version":1,"operation":"plan","environment":input,"output":output,"write":true});
    let bytes = serde_json::to_vec(&request).unwrap();
    assert!(matches!(
        private_command(&bytes, &root_dir()).unwrap(),
        PerformanceIdentitiesCommand::Plan { .. }
    ));
    for invalid in [
        br#"{"format_version":1,"format_\u0076ersion":2}"#.to_vec(),
        vec![0xff],
        vec![b' '; 32 * 1024 + 1],
        b"[]".to_vec(),
        b"{}".to_vec(),
    ] {
        assert!(private_command(&invalid, &root_dir()).is_err());
    }
    for (key, replacement) in [
        ("write", json!(false)),
        ("operation", json!("reconcile")),
        ("environment", json!("relative.json")),
        ("extra", json!("private-secret")),
    ] {
        let mut invalid = request.clone();
        invalid[key] = replacement;
        assert!(private_command(&serde_json::to_vec(&invalid).unwrap(), &root_dir()).is_err());
    }
    assert!(!output.exists());
}

fn complete_state(plan: &Value) -> Value {
    let mut state = json!({"format_version":1,"plan_sha256":plan["plan_sha256"],"status":"prepared","entries":[],"roles":{},"users":{}});
    let mut sequence = 10;
    for (operation, slot) in native_state::expected(plan).unwrap() {
        let id = match operation.as_str() {
            "post_system_roles" => {
                sequence += 1;
                state["roles"][&slot] = json!(sequence.to_string());
                sequence.to_string()
            }
            "post_system_users" => {
                sequence += 1;
                state["users"][&slot] = json!(sequence.to_string());
                sequence.to_string()
            }
            "put_system_roles_by_id_permissions" => state["roles"][&slot].as_str().unwrap().into(),
            _ => state["users"][&slot].as_str().unwrap().into(),
        };
        let mut entry = json!({"operation":operation,"slot":slot,"phase":"confirmed","id":id});
        if operation == "post_system_users_by_id_password_reset_requests" {
            sequence += 1;
            entry["request_id"] = json!(sequence.to_string());
        }
        state["entries"].as_array_mut().unwrap().push(entry);
    }
    state
}

#[test]
fn native_identity_ledger_requires_all_six_hundred_twenty_two_confirmed_writes() {
    let fixture = Fixture::new();
    let plan = create_plan(&environment_fixture(&fixture), &fixture).unwrap();
    let state = complete_state(&plan);
    assert_eq!(state["entries"].as_array().unwrap().len(), 622);
    assert_eq!(state["roles"].as_object().unwrap().len(), 11);
    assert_eq!(state["users"].as_object().unwrap().len(), 200);
    native_state::validate(&plan, &state, true).unwrap();
    for (pointer, replacement) in [
        ("/entries/3/phase", json!("started")),
        ("/entries/3/id", json!("9223372036854775808")),
        ("/entries/3/id", json!("001")),
        ("/entries/1/id", json!("99999")),
        ("/entries/4/id", json!("99999")),
        ("/entries/4/slot", json!("other-user")),
        ("/entries/4/operation", json!("post_system_users")),
        ("/status", json!("needs-reconciliation")),
        ("/plan_sha256", json!("a".repeat(64))),
    ] {
        let mut invalid = state.clone();
        *invalid.pointer_mut(pointer).unwrap() = replacement;
        assert!(
            native_state::validate(&plan, &invalid, true).is_err(),
            "{pointer}"
        );
    }
    let mut reordered = state.clone();
    reordered["entries"].as_array_mut().unwrap().swap(3, 4);
    assert!(native_state::validate(&plan, &reordered, true).is_err());
    let mut incomplete = state.clone();
    incomplete["entries"].as_array_mut().unwrap().pop();
    assert!(native_state::validate(&plan, &incomplete, true).is_err());
    let mut missing = state;
    missing["users"]
        .as_object_mut()
        .unwrap()
        .remove("system-user-001");
    assert!(native_state::validate(&plan, &missing, true).is_err());
}

#[test]
fn native_ledger_checks_prefix_recovery_and_unique_reset_requests() {
    let fixture = Fixture::new();
    let plan = create_plan(&environment_fixture(&fixture), &fixture).unwrap();
    let mut prefix = complete_state(&plan);
    prefix["status"] = json!("applying");
    prefix["entries"].as_array_mut().unwrap().truncate(2);
    prefix["users"] = json!({});
    prefix["roles"]
        .as_object_mut()
        .unwrap()
        .retain(|slot, _| slot == "system");
    native_state::validate(&plan, &prefix, false).unwrap();
    assert!(native_state::validate(&plan, &prefix, true).is_err());
    let mut duplicates = complete_state(&plan);
    duplicates["entries"][6]["request_id"] = duplicates["entries"][3]["request_id"].clone();
    assert!(native_state::validate(&plan, &duplicates, true).is_err());
    let mut verifying = complete_state(&plan);
    verifying["status"] = json!("verifying");
    assert!(native_state::validate(&plan, &verifying, true).is_err());
    verifying["publication_nonce"] = json!("7a347df2-a5b4-4dc7-8e36-edee1dc39a48");
    native_state::validate(&plan, &verifying, true).unwrap();
    verifying["status"] = json!("verified");
    verifying["verified_receipt"] = json!("../other.json");
    assert!(native_state::validate(&plan, &verifying, true).is_err());
    verifying["verified_receipt"] = json!("verified-7a347df2-a5b4-5dc7-8e36-edee1dc39a48.json");
    native_state::validate(&plan, &verifying, true).unwrap();
}

#[cfg(windows)]
fn create_directory_link(target: &Path, link: &Path) {
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .status()
        .unwrap();
    assert!(status.success());
}

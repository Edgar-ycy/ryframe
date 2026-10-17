use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::{Value, json};

use super::ci::resource_gate::replay::{
    ProcessEnvironment, ToolExecutables, execute_configured, write_report,
};

const CATEGORIES: [&str; 7] = [
    "addition",
    "field",
    "permission",
    "relation",
    "sql",
    "rename",
    "delete",
];

struct FixtureOutput {
    _root: tempfile::TempDir,
    backend: PathBuf,
    work_dir: PathBuf,
    tool_log: PathBuf,
    document: Value,
}

#[test]
fn resource_gate_replay_executes_twenty_cases_with_zero_divergence() {
    let output = run_twenty_case_fixture();
    assert_report(&output.document);
    assert_cases(&output.document);
    assert_cleanup(&output);
}

fn run_twenty_case_fixture() -> FixtureOutput {
    let fixture = tempfile::tempdir().unwrap();
    let backend = fixture.path().join("repository with space");
    let frontend = fixture.path().join("前端 repository");
    let tools = fixture.path().join("tools");
    fs::create_dir(&backend).unwrap();
    fs::create_dir(&frontend).unwrap();
    fs::create_dir(&tools).unwrap();
    initialize_repository(&backend, &frontend);
    let cases = create_cases(&backend, &frontend);
    let tool_log = fixture.path().join("tool.log");
    let executables = create_fake_tools(&tools, &tool_log);
    let manifest = fixture.path().join("manifest.json");
    let current_exe = env::current_exe().unwrap();
    let targeted_wrapper = create_fixture_wrapper(
        &tools,
        "targeted",
        &current_exe,
        "resource_gate_replay_fixtures::targeted_fixture",
    );
    let full_wrapper = create_fixture_wrapper(
        &tools,
        "full",
        &current_exe,
        "resource_gate_replay_fixtures::full_fixture",
    );
    fs::write(
        &manifest,
        serde_json::to_vec_pretty(&json!({
            "formatVersion": 2,
            "targetedCommand": [targeted_wrapper],
            "fullCommand": [full_wrapper],
            "cases": cases,
        }))
        .unwrap(),
    )
    .unwrap();
    let work_dir = backend.join(".local-tests/resource-gate-replay/tests");
    let report = backend.join(".local-tests/resource-gate-replay/report.json");
    let mut environment = base_environment();
    environment.insert("RYFRAME_REPLAY_FIXTURE".to_owned(), "1".into());
    environment.insert(
        "RYFRAME_REPLAY_TEST_TOOL_LOG".to_owned(),
        tool_log.as_os_str().to_owned(),
    );
    let run = execute_configured(
        backend.clone(),
        frontend.clone(),
        manifest,
        work_dir.clone(),
        report.clone(),
        false,
        &environment,
        &executables,
    )
    .unwrap();
    write_report(&report, &run, false).unwrap();

    let document = serde_json::from_slice::<Value>(&fs::read(&report).unwrap()).unwrap();
    FixtureOutput {
        _root: fixture,
        backend,
        work_dir,
        tool_log,
        document,
    }
}

fn assert_report(document: &Value) {
    assert_eq!(document["caseCount"], 20);
    assert_eq!(document["successfulTargetedCaseCount"], 10);
    assert_eq!(document["zeroDivergence"], true);
    assert_eq!(document["activationEligible"], false);
    assert_eq!(document["replayCoverageEligible"], true);
    assert_eq!(document["targetedWithinBudget"], true);
    assert!(document["targetedP95Ms"].as_u64().unwrap() <= 60_000);
    assert_eq!(document["sccacheCacheErrors"], Value::Null);
    assert_eq!(document["sccacheHealthy"], true);
    assert_eq!(
        document["evidence"]["environment"]["CARGO_INCREMENTAL"],
        "0"
    );
    assert_eq!(
        document["evidence"]["environment"]["RYFRAME_CI_RUST_GATE_PROFILE"],
        "standard"
    );
    assert_eq!(document["evidence"]["tools"]["sccache"], "sccache 0.test");
    assert!(
        document["evidence"]["manifest_fingerprint"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    assert!(
        document["evidence"]["runner_fingerprint"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
}

fn assert_cases(document: &Value) {
    assert_eq!(document["evidence"]["primes"].as_array().unwrap().len(), 1);
    let prime = &document["evidence"]["primes"][0];
    assert_eq!(prime["result"]["order"], 0);
    assert_eq!(prime["change_count"], 4);
    assert_eq!(prime["rust_crates"], json!([]));
    assert_eq!(document["cases"][0]["targeted"]["order"], 1);
    assert_eq!(document["cases"][0]["full"]["order"], 2);
    assert_eq!(document["cases"][1]["full"]["order"], 3);
    assert_eq!(document["cases"][1]["targeted"]["order"], 4);
    assert!(document["cases"].as_array().unwrap().iter().all(|case| {
        case["matches"] == true
            && case["targeted"]["decision"]["recognized"] == true
            && case["full"]["decision"]["recognized"] == false
            && case["full"]["decision"]["mode"] == "full"
    }));
    let categories = document["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| case["category"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(categories, CATEGORIES.into_iter().collect());
    let targets = document["cases"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|case| [&case["targeted"], &case["full"]])
        .map(|arm| arm["target_fingerprint"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(targets.len(), 1);
}

fn assert_cleanup(output: &FixtureOutput) {
    let installs = fs::read_to_string(&output.tool_log)
        .unwrap()
        .lines()
        .filter(|line| *line == "pnpm install --offline --frozen-lockfile")
        .count();
    assert_eq!(installs, 1);
    assert_eq!(fs::read_dir(&output.work_dir).unwrap().count(), 0);
    let worktrees = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(&output.backend)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&worktrees.stdout)
            .lines()
            .filter(|line| line.starts_with("worktree "))
            .count(),
        1
    );
}

#[test]
fn targeted_fixture() {
    run_driver("targeted");
}

#[test]
fn full_fixture() {
    run_driver("full");
}

fn run_driver(mode: &str) {
    if env::var("RYFRAME_REPLAY_FIXTURE").ok().as_deref() != Some("1") {
        return;
    }
    let cwd = env::current_dir().unwrap();
    let frontend =
        PathBuf::from(env::var_os("RYFRAME_REPLAY_FIXTURE_FRONTEND").expect("fixture 缺少前端"));
    assert!(frontend.is_absolute());
    assert!(frontend.join("node_modules").is_dir());
    assert_eq!(
        fs::read_to_string(frontend.join("scenario.txt")).unwrap(),
        fs::read_to_string("scenario.txt").unwrap()
    );
    let target = PathBuf::from(env::var_os("RYFRAME_DEVEX_TARGET_ROOT").unwrap());
    assert!(target.is_absolute());
    assert_eq!(target.file_name().unwrap(), "target");
    assert_eq!(
        target.parent().unwrap(),
        cwd.parent().unwrap().parent().unwrap()
    );
    let runner_target = PathBuf::from(env::var_os("CARGO_TARGET_DIR").unwrap());
    assert_eq!(runner_target, target.join("runner"));
    let activation = env::var("RYFRAME_RESOURCE_GATE_TARGETED").ok();
    let actual_mode = if mode == "targeted" {
        assert_eq!(activation.as_deref(), Some("replay-verified-v1"));
        fs::read_to_string("targeted-mode.txt")
            .unwrap()
            .trim()
            .to_owned()
    } else {
        assert!(activation.is_none());
        "full".to_owned()
    };
    let status = fs::read_to_string("status.txt").unwrap();
    let decision = json!({
        "formatVersion": 1,
        "recognized": mode == "targeted",
        "mode": actual_mode,
        "fallback": if actual_mode == "targeted" {
            Value::Null
        } else {
            Value::String("fixture full fallback".to_owned())
        },
        "steps": if actual_mode == "targeted" {
            json!(["resource-drift", "resource-workspace"])
        } else {
            json!(["resource-drift", "full-rust-gate"])
        }
    });
    fs::write(
        PathBuf::from(env::var_os("RYFRAME_RESOURCE_GATE_DECISION_FILE").unwrap()),
        serde_json::to_vec(&decision).unwrap(),
    )
    .unwrap();
    assert_eq!(status.trim(), "pass", "fixture 预期失败");
}

fn initialize_repository(backend: &Path, frontend: &Path) {
    git(backend, &["init", "--quiet"]);
    git(backend, &["config", "user.email", "ci@example.invalid"]);
    git(backend, &["config", "user.name", "CI"]);
    git(backend, &["config", "core.autocrlf", "false"]);
    fs::write(
        backend.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::write(backend.join("status.txt"), "pass\n").unwrap();
    fs::write(backend.join("scenario.txt"), "base\n").unwrap();
    fs::write(backend.join("targeted-mode.txt"), "targeted\n").unwrap();
    git(backend, &["add", "."]);
    git(backend, &["commit", "--quiet", "-m", "fixture base"]);

    git(frontend, &["init", "--quiet"]);
    git(frontend, &["config", "user.email", "ci@example.invalid"]);
    git(frontend, &["config", "user.name", "CI"]);
    git(frontend, &["config", "core.autocrlf", "false"]);
    fs::write(frontend.join(".gitignore"), "node_modules/\n").unwrap();
    fs::write(
        frontend.join("package.json"),
        r#"{"name":"resource-replay-fixture","private":true,"packageManager":"pnpm@11.20.0"}"#,
    )
    .unwrap();
    fs::write(frontend.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();
    fs::write(frontend.join("scenario.txt"), "base\n").unwrap();
    git(frontend, &["add", "."]);
    git(frontend, &["commit", "--quiet", "-m", "fixture base"]);
}

fn create_cases(backend: &Path, frontend: &Path) -> Vec<Value> {
    (0..20)
        .map(|index| {
            let base = git_output(backend, &["rev-parse", "HEAD"]);
            let frontend_base = git_output(frontend, &["rev-parse", "HEAD"]);
            let category = CATEGORIES[index % CATEGORIES.len()];
            let status = if index % 2 == 0 { "pass" } else { "fail" };
            let targeted_mode = if index % 2 == 0 { "targeted" } else { "full" };
            let name = format!("{category}-{index}");
            fs::write(backend.join("status.txt"), format!("{status}\n")).unwrap();
            fs::write(backend.join("scenario.txt"), format!("{name}\n")).unwrap();
            fs::write(
                backend.join("targeted-mode.txt"),
                format!("{targeted_mode}\n"),
            )
            .unwrap();
            git(
                backend,
                &["add", "status.txt", "scenario.txt", "targeted-mode.txt"],
            );
            git(
                backend,
                &[
                    "commit",
                    "--quiet",
                    "-m",
                    &format!("{category} case {index}"),
                ],
            );
            fs::write(frontend.join("scenario.txt"), format!("{name}\n")).unwrap();
            git(frontend, &["add", "scenario.txt"]);
            git(
                frontend,
                &["commit", "--quiet", "-m", &format!("{name} frontend")],
            );
            json!({
                "name": name,
                "category": category,
                "base": base,
                "head": git_output(backend, &["rev-parse", "HEAD"]),
                "frontendBase": frontend_base,
                "frontendHead": git_output(frontend, &["rev-parse", "HEAD"]),
                "targetedMode": targeted_mode,
                "expected": status,
            })
        })
        .collect()
}

fn create_fake_tools(directory: &Path, log: &Path) -> ToolExecutables {
    let corepack = tool_script(
        directory,
        "corepack",
        r#"@echo off
>>"%RYFRAME_REPLAY_TEST_TOOL_LOG%" echo %*
if "%~1"=="--version" (
  echo corepack 0.test
  exit /b 0
)
if "%~1"=="pnpm" if "%~2"=="--version" (
  echo 11.20.0-test
  exit /b 0
)
if "%~1"=="pnpm" if "%~2"=="install" (
  mkdir node_modules 2>nul
  exit /b 0
)
exit /b 2
"#,
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$RYFRAME_REPLAY_TEST_TOOL_LOG"
if [ "$1" = "--version" ]; then echo 'corepack 0.test'; exit 0; fi
if [ "$1" = "pnpm" ] && [ "$2" = "--version" ]; then echo '11.20.0-test'; exit 0; fi
if [ "$1" = "pnpm" ] && [ "$2" = "install" ]; then mkdir -p node_modules; exit 0; fi
exit 2
"#,
    );
    let sccache = tool_script(
        directory,
        "sccache",
        r#"@echo off
>>"%RYFRAME_REPLAY_TEST_TOOL_LOG%" echo %*
if "%~1"=="--version" (
  echo sccache 0.test
  exit /b 0
)
if "%~1"=="--show-stats" (
  echo {"compile_requests":0,"stats":{"cache_errors":{"counts":{}},"cache_timeouts":0,"cache_read_errors":0,"cache_write_errors":0,"dist_errors":0}}
  exit /b 0
)
exit /b 0
"#,
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$RYFRAME_REPLAY_TEST_TOOL_LOG"
if [ "$1" = "--version" ]; then echo 'sccache 0.test'; exit 0; fi
if [ "$1" = "--show-stats" ]; then echo '{"compile_requests":0,"stats":{"cache_errors":{"counts":{}},"cache_timeouts":0,"cache_read_errors":0,"cache_write_errors":0,"dist_errors":0}}'; exit 0; fi
exit 0
"#,
    );
    let _ = log;
    ToolExecutables { corepack, sccache }
}

fn tool_script(directory: &Path, name: &str, windows: &str, unix: &str) -> PathBuf {
    let _ = (windows, unix);
    #[cfg(windows)]
    {
        let path = directory.join(format!("{name}.cmd"));
        fs::write(&path, windows.replace('\n', "\r\n")).unwrap();
        path
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;

        let path = directory.join(name);
        fs::write(&path, unix).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }
}

fn create_fixture_wrapper(directory: &Path, label: &str, executable: &Path, test: &str) -> PathBuf {
    #[cfg(windows)]
    {
        let path = directory.join(format!("{label}-fixture.cmd"));
        fs::write(
            &path,
            format!(
                "@echo off\r\nsetlocal\r\nset \"FRONTEND=\"\r\n:parse\r\nif \"%~1\"==\"\" goto run\r\nif \"%~1\"==\"--frontend-dir\" (\r\n  set \"FRONTEND=%~2\"\r\n  shift\r\n)\r\nshift\r\ngoto parse\r\n:run\r\nset \"RYFRAME_REPLAY_FIXTURE_FRONTEND=%FRONTEND%\"\r\n\"{}\" --exact {} --nocapture\r\nexit /b %ERRORLEVEL%\r\n",
                executable.display(),
                test
            ),
        )
        .unwrap();
        path
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;

        let path = directory.join(format!("{label}-fixture"));
        fs::write(
            &path,
            format!(
                "#!/bin/sh\nfrontend=\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"--frontend-dir\" ]; then frontend=\"$2\"; shift 2; else shift; fi\ndone\nRYFRAME_REPLAY_FIXTURE_FRONTEND=\"$frontend\" exec '{}' --exact {} --nocapture\n",
                executable.display(),
                test
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }
}

fn base_environment() -> ProcessEnvironment {
    env::vars_os()
        .filter_map(|(key, value)| key.into_string().ok().map(|key| (key, value)))
        .collect()
}

fn git(repository: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(repository)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} 失败：{}",
        arguments.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_output(repository: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(repository)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

use std::{
    env, fs,
    io::{Read, Write},
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
    let environment = base_environment();
    let run = execute_configured(
        backend.clone(),
        frontend,
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
    assert!(
        document["evidence"]["environment"]
            .get("RYFRAME_CI_RUST_GATE_PROFILE")
            .is_none()
    );
    assert!(
        document["evidence"]["environment"]
            .get("RYFRAME_CI_TEST_JOBS")
            .is_none()
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
    for case in document["cases"].as_array().unwrap() {
        for arm in ["targeted", "full"] {
            let result = &case[arm];
            if case["expected"] == "fail" {
                assert_ne!(result["return_code"], 0);
                assert!(
                    result["failure_tail"]
                        .as_str()
                        .unwrap()
                        .contains("fixture 预期失败")
                );
            } else {
                assert_eq!(result["return_code"], 0);
                assert!(result["failure_tail"].is_null());
            }
        }
    }
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
#[ignore = "仅由 replay 驱动通过受限 stdin 请求调用"]
fn targeted_fixture() {
    run_driver("targeted");
}

#[test]
#[ignore = "仅由 replay 驱动通过受限 stdin 请求调用"]
fn full_fixture() {
    run_driver("full");
}

fn run_driver(mode: &str) {
    let request = read_driver_request(std::io::stdin().lock()).unwrap();
    let cwd = env::current_dir().unwrap();
    let arguments = fs::read_to_string("fixture-arguments.txt").unwrap();
    assert!(
        arguments.trim().is_empty(),
        "外部夹具不得被追加资源门禁参数"
    );
    let frontend = cwd.parent().unwrap().join("f");
    assert!(frontend.is_absolute());
    assert!(frontend.join("node_modules").is_dir());
    assert_eq!(
        fs::read_to_string(frontend.join("scenario.txt")).unwrap(),
        fs::read_to_string("scenario.txt").unwrap()
    );
    let target = request.target_root;
    assert!(target.is_absolute());
    assert_eq!(target.file_name().unwrap(), "target");
    assert_eq!(
        target.parent().unwrap(),
        cwd.parent().unwrap().parent().unwrap()
    );
    let runner_target = PathBuf::from(env::var_os("CARGO_TARGET_DIR").unwrap());
    assert_eq!(runner_target, target.join("runner"));
    let actual_mode = if mode == "targeted" {
        fs::read_to_string("targeted-mode.txt")
            .unwrap()
            .trim()
            .to_owned()
    } else {
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
        cwd.parent().unwrap().join("decision.json"),
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
    ToolExecutables {
        corepack: corepack_fixture(directory, log),
        sccache: sccache_fixture(directory, log),
    }
}

fn corepack_fixture(directory: &Path, log: &Path) -> PathBuf {
    tool_script(
        directory,
        "corepack",
        log,
        r#"@echo off
if "%*"=="--version" (
  @version@
  echo corepack 0.test
  exit /b 0
)
if "%*"=="pnpm --version" (
  @pnpm-version@
  echo 11.20.0-test
  exit /b 0
)
if "%*"=="pnpm install --offline --frozen-lockfile" (
  @install@
  mkdir node_modules 2>nul
  exit /b 0
)
exit /b 2
"#,
        r#"#!/bin/sh
if [ "$*" = "--version" ]; then @version@; echo 'corepack 0.test'; exit 0; fi
if [ "$*" = "pnpm --version" ]; then @pnpm-version@; echo '11.20.0-test'; exit 0; fi
if [ "$*" = "pnpm install --offline --frozen-lockfile" ]; then @install@; mkdir -p node_modules; exit 0; fi
exit 2
"#,
    )
}

fn sccache_fixture(directory: &Path, log: &Path) -> PathBuf {
    tool_script(
        directory,
        "sccache",
        log,
        r#"@echo off
if "%*"=="--version" (
  @version@
  echo sccache 0.test
  exit /b 0
)
if "%*"=="--show-stats --stats-format json" (
  @stats@
  echo {"compile_requests":0,"stats":{"cache_errors":{"counts":{}},"cache_timeouts":0,"cache_read_errors":0,"cache_write_errors":0,"dist_errors":0}}
  exit /b 0
)
if "%*"=="--start-server" (
  @start@
  exit /b 0
)
if "%*"=="--zero-stats" (
  @zero@
  exit /b 0
)
if "%*"=="--stop-server" (
  @stop@
  exit /b 0
)
exit /b 2
"#,
        r#"#!/bin/sh
if [ "$*" = "--version" ]; then @version@; echo 'sccache 0.test'; exit 0; fi
if [ "$*" = "--show-stats --stats-format json" ]; then @stats@; echo '{"compile_requests":0,"stats":{"cache_errors":{"counts":{}},"cache_timeouts":0,"cache_read_errors":0,"cache_write_errors":0,"dist_errors":0}}'; exit 0; fi
if [ "$*" = "--start-server" ]; then @start@; exit 0; fi
if [ "$*" = "--zero-stats" ]; then @zero@; exit 0; fi
if [ "$*" = "--stop-server" ]; then @stop@; exit 0; fi
exit 2
"#,
    )
}

fn tool_script(directory: &Path, name: &str, log: &Path, windows: &str, unix: &str) -> PathBuf {
    let (windows, unix) = bind_tool_log_requests(directory, name, log, windows, unix);
    let _ = (&windows, &unix);
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
        fs::write(&path, &unix).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolLogRequest {
    format_version: u32,
    log: PathBuf,
    entry: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DriverRequest {
    format_version: u32,
    target_root: PathBuf,
}

fn read_json_request<T: serde::de::DeserializeOwned>(reader: impl Read) -> Result<T, String> {
    let mut bytes = Vec::new();
    reader
        .take(16 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > 16 * 1024 {
        return Err("夹具 JSON 请求超过 16 KiB".to_owned());
    }
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

fn read_driver_request(reader: impl Read) -> Result<DriverRequest, String> {
    let request: DriverRequest = read_json_request(reader)?;
    if request.format_version != 1 || !request.target_root.is_absolute() {
        return Err("驱动请求的版本或 target_root 绝对路径无效".to_owned());
    }
    Ok(request)
}

fn read_tool_log_request(reader: impl Read) -> Result<ToolLogRequest, String> {
    let request: ToolLogRequest = read_json_request(reader)?;
    if request.format_version != 1
        || !request.log.is_absolute()
        || request.entry.contains(['\r', '\n'])
    {
        return Err("工具日志请求的版本、绝对路径或单行记录无效".to_owned());
    }
    Ok(request)
}

#[test]
#[ignore = "仅由假工具通过受管 JSON 请求调用"]
fn tool_log_fixture() {
    let request = read_tool_log_request(std::io::stdin().lock()).unwrap();
    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(request.log)
        .unwrap();
    writeln!(log, "{}", request.entry).unwrap();
}

fn bind_tool_log_requests(
    directory: &Path,
    name: &str,
    log: &Path,
    windows: &str,
    unix: &str,
) -> (String, String) {
    let mut windows = windows.to_owned();
    let mut unix = unix.to_owned();
    let executable = env::current_exe().unwrap();
    for (token, entry) in [
        ("version", "--version"),
        ("pnpm-version", "pnpm --version"),
        ("install", "pnpm install --offline --frozen-lockfile"),
        ("stats", "--show-stats --stats-format json"),
        ("start", "--start-server"),
        ("zero", "--zero-stats"),
        ("stop", "--stop-server"),
    ] {
        let token_text = format!("@{token}@");
        if !windows.contains(&token_text) && !unix.contains(&token_text) {
            continue;
        }
        let path = directory.join(format!("{name}-{token}.json"));
        fs::write(
            &path,
            serde_json::to_vec(&ToolLogRequest {
                format_version: 1,
                log: log.to_path_buf(),
                entry: entry.to_owned(),
            })
            .unwrap(),
        )
        .unwrap();
        let helper =
            "--exact resource_gate_replay_fixtures::tool_log_fixture --ignored --nocapture";
        windows = windows.replace(
            &token_text,
            &format!(
                "\"{}\" {helper} < \"{}\" >nul || exit /b 97",
                executable.display().to_string().replace('%', "%%"),
                path.display().to_string().replace('%', "%%"),
            ),
        );
        unix = unix.replace(
            &token_text,
            &format!(
                "{} {helper} < {} >/dev/null || exit 97",
                shell_quote(&executable),
                shell_quote(&path),
            ),
        );
    }
    (windows, unix)
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
}

#[test]
fn tool_log_protocol_rejects_invalid_and_oversized_requests() {
    for input in [
        "",
        "{}",
        "null",
        r#"{"format_version":2,"log":"/tmp/log","entry":"test"}"#,
        r#"{"format_version":1,"log":"relative","entry":"test"}"#,
        r#"{"format_version":1,"log":"/tmp/log","entry":"a\nb"}"#,
        r#"{"format_version":1,"log":"/tmp/log","entry":"test","extra":true}"#,
    ] {
        assert!(read_tool_log_request(input.as_bytes()).is_err());
    }
    assert!(
        read_tool_log_request(&vec![b' '; 16 * 1024 + 1][..])
            .unwrap_err()
            .contains("16 KiB")
    );
}

#[test]
fn driver_protocol_rejects_invalid_and_oversized_requests() {
    for input in [
        "",
        "{}",
        "null",
        "{",
        r#"{"format_version":2,"target_root":"/tmp"}"#,
        r#"{"format_version":1,"target_root":"relative"}"#,
        r#"{"format_version":1,"target_root":"/tmp","extra":true}"#,
        r#"{"format_version":1,"target_root":"/tmp"} {}"#,
    ] {
        assert!(read_driver_request(input.as_bytes()).is_err());
    }
    assert!(
        read_driver_request(&vec![b' '; 16 * 1024 + 1][..])
            .unwrap_err()
            .contains("16 KiB")
    );
}

fn create_fixture_wrapper(directory: &Path, label: &str, executable: &Path, test: &str) -> PathBuf {
    #[cfg(windows)]
    {
        let path = directory.join(format!("{label}-fixture.cmd"));
        fs::write(
            &path,
            format!(
                "@echo off\r\nsetlocal\r\ntype nul > fixture-arguments.txt\r\n:parse\r\nif \"%~1\"==\"\" goto run\r\n>>fixture-arguments.txt echo %~1\r\nshift\r\ngoto parse\r\n:run\r\n\"{}\" --exact {} --ignored --nocapture\r\nexit /b %ERRORLEVEL%\r\n",
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
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > fixture-arguments.txt\nexec '{}' --exact {} --ignored --nocapture\n",
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

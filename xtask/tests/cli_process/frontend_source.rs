use super::{invoke, xtask_command};

#[test]
fn frontend_source_validates_all_inputs_before_execution() {
    let event = std::env::current_dir()
        .unwrap()
        .join("target/CI event.json")
        .to_string_lossy()
        .into_owned();
    let candidate = std::env::current_dir()
        .unwrap()
        .join("target/candidate OpenAPI.json")
        .to_string_lossy()
        .into_owned();
    let invalid = [
        vec!["check", "ci", "frontend-source"],
        vec![
            "check",
            "ci",
            "frontend-source",
            "--event-name",
            "push",
            "--release-ref",
        ],
        vec![
            "check",
            "ci",
            "frontend-source",
            "--event-name",
            "push",
            "--event-name",
            "push",
        ],
        vec![
            "check",
            "ci",
            "frontend-source",
            "--event-name",
            "push",
            "--backend-worktree",
            ".",
        ],
        vec![
            "check",
            "ci",
            "frontend-source",
            "--event-name",
            "pull_request",
            "--event",
            event.as_str(),
            "--base-sha",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        ],
        vec![
            "check",
            "ci",
            "frontend-source",
            "--event-name",
            "push",
            "--release-ref",
            "refs/heads/main\nunsafe",
        ],
        vec![
            "check",
            "ci",
            "frontend-source",
            "--event-name",
            "pull_request",
            "--event",
            event.as_str(),
            "--base-sha",
            "0123456789abcdef0123456789abcdef01234567",
            "--candidate-openapi",
            candidate.as_str(),
            "--fallback-main-on-invalid-base",
        ],
    ];
    for arguments in invalid {
        let result = invoke(&arguments);
        assert_eq!(result.status.code(), Some(2), "参数：{arguments:?}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("参数错误"));
        assert!(result.stdout.is_empty());
    }
}

#[test]
fn frontend_source_push_runs_the_native_implementation() {
    let directory = frontend_repository_workspace();
    let result = xtask_command()
        .current_dir(directory.path())
        .args([
            "check",
            "ci",
            "frontend-source",
            "--event-name",
            "push",
            "--base-sha",
            "",
            "--release-ref",
            "refs/heads/main",
        ])
        .env("GITHUB_REPOSITORY_OWNER", "ExampleOwner")
        .env_remove("GITHUB_OUTPUT")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8(result.stdout).unwrap();
    assert!(
        stdout.contains("开始 CI 原子任务：ci.frontend-source"),
        "{stdout}"
    );
    assert!(stdout.contains("ref=main"), "{stdout}");
    assert!(stdout.contains("changed=false"), "{stdout}");
    assert!(
        stdout.contains("repository=ExampleOwner/ryframe-vue3"),
        "{stdout}"
    );
}

#[test]
fn frontend_source_appends_the_github_output_contract() {
    let directory = frontend_repository_workspace();
    let output_path = directory.path().join("github-output.txt");
    std::fs::write(&output_path, "previous=kept\n").unwrap();
    let result = xtask_command()
        .current_dir(directory.path())
        .args([
            "check",
            "ci",
            "frontend-source",
            "--event-name",
            "push",
            "--release-ref",
            "refs/heads/main",
        ])
        .env("GITHUB_OUTPUT", &output_path)
        .env("GITHUB_REPOSITORY_OWNER", "ExampleOwner")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(output_path).unwrap(),
        "previous=kept\nref=main\nchanged=false\nrepository=ExampleOwner/ryframe-vue3\n"
    );
}

#[test]
fn frontend_source_rejects_a_missing_event_before_selection() {
    let event = std::env::current_dir()
        .unwrap()
        .join("target/missing frontend event.json")
        .to_string_lossy()
        .into_owned();
    let result = invoke(&[
        "check",
        "ci",
        "frontend-source",
        "--event-name",
        "pull_request",
        "--event",
        &event,
        "--base-sha",
        "0123456789abcdef0123456789abcdef01234567",
    ]);
    assert_eq!(result.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!stdout.contains("→ python"), "{stdout}");
    assert!(stderr.contains("无法读取前端来源事件文件"), "{stderr}");
}

#[test]
fn frontend_source_plan_uses_the_registered_graph_without_reading_runtime_inputs() {
    let event = std::env::current_dir()
        .unwrap()
        .join("target/不存在的前端来源 event.json")
        .to_string_lossy()
        .into_owned();
    let directory = frontend_repository_workspace();
    std::fs::write(
        directory.path().join("config/tools.toml"),
        "[workspace]\nfrontend_repository=''\n",
    )
    .unwrap();
    let output_path = directory.path().join("job-output.txt");
    let result = xtask_command()
        .current_dir(directory.path())
        .args([
            "check",
            "ci",
            "frontend-source",
            "--event-name",
            "pull_request",
            "--event",
            &event,
            "--base-sha",
            "0123456789abcdef0123456789abcdef01234567",
            "--plan",
        ])
        .env_remove("GITHUB_REPOSITORY_OWNER")
        .env("GITHUB_OUTPUT", &output_path)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8(result.stdout).unwrap();
    assert!(stdout.contains("ci.frontend-source"), "{stdout}");
    assert!(!stdout.contains("开始 CI 原子任务"), "{stdout}");
    assert!(!output_path.exists());
    assert!(!stdout.contains("→ python"), "{stdout}");
}

fn frontend_repository_workspace() -> tempfile::TempDir {
    let base = std::env::current_dir().unwrap().join(".local-tests");
    std::fs::create_dir_all(&base).unwrap();
    let directory = tempfile::tempdir_in(base).unwrap();
    std::fs::create_dir(directory.path().join("xtask")).unwrap();
    std::fs::create_dir(directory.path().join("config")).unwrap();
    std::fs::write(directory.path().join("Cargo.toml"), "[workspace]\n").unwrap();
    directory
}

#[test]
fn frontend_source_missing_owner_does_not_use_legacy_repository_environment_or_write_output() {
    let directory = frontend_repository_workspace();
    let output_path = directory.path().join("job-output.txt");
    let result = xtask_command()
        .current_dir(directory.path())
        .args(["check", "ci", "frontend-source", "--event-name", "push"])
        .env_remove("GITHUB_REPOSITORY_OWNER")
        .env("FRONTEND_REPOSITORY", "legacy/frontend")
        .env("RYFRAME_FRONTEND_REPOSITORY", "legacy/override")
        .env("GITHUB_OUTPUT", &output_path)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(stderr.contains("workspace.frontend_repository"), "{stderr}");
    assert!(stderr.contains("config/tools.toml"), "{stderr}");
    assert!(!output_path.exists());
}

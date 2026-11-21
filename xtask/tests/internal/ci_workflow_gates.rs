use super::*;

#[test]
fn edited_pull_requests_keep_consumer_and_security_while_skipping_windows_replay() {
    let ci = workflow("ci.yml");
    assert!(
        ci.get("on")
            .unwrap()
            .get("pull_request")
            .unwrap()
            .get("types")
            .unwrap()
            .as_sequence()
            .unwrap()
            .iter()
            .any(|value| value.as_str() == Some("edited"))
    );
    let resource = job(&ci, "resource-gate");
    assert_eq!(
        text(resource, "if").split_whitespace().collect::<String>(),
        "${{needs.plan.result=='success'&&(needs.plan.outputs.resource_gate=='true'||needs.plan.outputs.consumer_contract=='true')}}"
    );
    let (_, consumer) = command(resource, "cargo xtask check ci consumer-contract");
    assert_eq!(
        text(consumer, "if").split_whitespace().collect::<String>(),
        "${{needs.plan.outputs.consumer_contract=='true'}}"
    );
    assert_eq!(
        text(job(&ci, "security-audit"), "if")
            .split_whitespace()
            .collect::<String>(),
        "${{needs.plan.result=='success'}}"
    );
    assert_eq!(
        text(job(&ci, "windows-smoke"), "if")
            .split_whitespace()
            .collect::<String>(),
        "${{needs.plan.result=='success'&&github.event.action!='edited'}}"
    );
    let (_, windows) = command(job(&ci, "windows-smoke"), "cargo xtask check ci rust-gate");
    let tokens: Vec<_> = text(windows, "run").split_whitespace().collect();
    assert!(
        tokens
            .windows(2)
            .any(|args| args == ["--profile", "windows-smoke"])
    );
}

#[test]
fn security_source_precedes_conditional_image_build_and_verification() {
    let ci = workflow("ci.yml");
    let job = job(&ci, "security-audit");
    let (source, _) = command(job, "cargo xtask check ci security source");
    let (deployment, decision) = command(job, "cargo xtask check ci security deployment source");
    let (build, image) = command(job, "docker build");
    let (verify, check) = command(job, "cargo xtask check ci security deployment image");
    assert!(source < deployment && deployment < build && build < verify);
    assert_eq!(text(decision, "id"), "deployment");
    assert_eq!(
        text(image, "if"),
        "steps.deployment.outputs.required == 'true'"
    );
    assert_eq!(text(check, "if"), text(image, "if"));
}

#[test]
fn full_stack_native_stages_keep_failure_collection_and_required_artifacts() {
    let extended = workflow("extended-ci.yml");
    assert!(extended.get("on").unwrap().get("schedule").is_some());
    assert!(
        extended
            .get("on")
            .unwrap()
            .get("workflow_dispatch")
            .is_some()
    );
    let job = job(&extended, "full-stack-e2e");
    let configured = assert_full_stack_configuration(job);
    let tool_target = "$RUNNER_TEMP/ryframe-full-stack-tool";
    let backend = "${{ matrix.backend }}";
    let (built, tool_build) = command(job, "cargo build --locked -p xtask --bin xtask");
    assert_eq!(argument(tool_build, "--target-dir"), tool_target);
    assert_eq!(text(tool_build, "working-directory"), backend);

    let native_stage = |mode: &str| {
        let invocation = format!(r#""{tool_target}/debug/xtask" check recovery full-stack {mode}"#);
        let (index, step) = command(job, &invocation);
        assert_eq!(text(step, "run").trim(), invocation);
        assert_eq!(text(step, "working-directory"), backend);
        (index, step)
    };
    let (prepare, _) = native_stage("prepare");
    let (start, _) = native_stage("start");
    let (core, core_frontend) = command(job, "corepack pnpm build");
    let (business, business_frontend) = native_stage("frontend");
    let (verify, verifier) = command(job, "cargo xtask check release ci verify-pair");
    let (collect, collector) = native_stage("collect");

    assert_eq!(text(core_frontend, "if"), "${{ matrix.fixture == 'core' }}");
    assert_eq!(
        text(core_frontend, "working-directory"),
        "${{ matrix.frontend }}"
    );
    assert_eq!(
        text(business_frontend, "if"),
        "${{ matrix.fixture == 'business' }}"
    );
    assert_eq!(
        argument(verifier, "--input"),
        "$RUNNER_TEMP/ryframe-full-stack/source-pair.json"
    );
    assert_eq!(
        argument(verifier, "--frontend-dir"),
        "$GITHUB_WORKSPACE/frontend"
    );
    assert!(configured < built && built < prepare && prepare < start);
    assert!(start < core && core < verify);
    assert!(start < business && business < verify);
    assert!(verify < collect);
    assert_eq!(text(collector, "if"), "${{ always() }}");
    let upload = steps(job)[collect + 1..]
        .iter()
        .find(|step| uses(step, "actions/upload-artifact@"))
        .unwrap();
    assert_eq!(text(upload, "if"), "${{ always() }}");
    let upload_inputs = upload.get("with").unwrap();
    assert_eq!(text(upload_inputs, "if-no-files-found"), "error");
    assert_eq!(
        upload_inputs.get("include-hidden-files"),
        Some(&YamlValue::Bool(true))
    );
    assert!(
        text(upload_inputs, "path")
            .lines()
            .any(|path| path.trim() == "${{ runner.temp }}/ryframe-full-stack")
    );
}

fn assert_full_stack_configuration(job: &YamlValue) -> usize {
    let (configured, configuration) = fixture_configuration(job);
    assert_eq!(configuration["api_docs"]["enabled"].as_bool(), Some(false));
    assert_eq!(
        configuration["reset"]["legacy_mysql_exclusive"].as_bool(),
        Some(true)
    );
    let (_, configuration_step) = command(job, "cargo xtask check ci full-stack-config");
    assert_eq!(
        argument(configuration_step, "--runtime-dir"),
        "$RUNNER_TEMP/ryframe-full-stack"
    );
    assert_eq!(
        argument(configuration_step, "--source-backend"),
        "$GITHUB_WORKSPACE/backend"
    );
    assert_eq!(
        argument(configuration_step, "--source-frontend"),
        "$GITHUB_WORKSPACE/frontend"
    );
    for (flag, expected) in [
        ("--run-id", "${{github.run_id}}"),
        ("--attempt", "${{github.run_attempt}}"),
        ("--source-sha", "${{github.sha}}"),
        ("--mysql-container", "${{job.services.mysql.id}}"),
        ("--redis-container", "${{job.services.redis.id}}"),
        ("--s3-container", "${{job.services.rustfs.id}}"),
    ] {
        assert_eq!(
            argument(configuration_step, flag)
                .split_whitespace()
                .collect::<String>(),
            expected
        );
    }
    assert!(
        text(configuration_step, "run")
            .split_whitespace()
            .any(|v| v == "--write")
    );
    configured
}

#[test]
fn service_file_configuration_precedes_its_gate_and_uses_only_app_env() {
    let ci = workflow("ci.yml");
    for name in ["resource-gate", "integration"] {
        let job = job(&ci, name);
        let env = job.get("env").unwrap();
        assert_eq!(env.as_mapping().unwrap().len(), 1);
        assert_eq!(text(env, "APP_ENV"), "test");
        let configuration = steps(job)
            .iter()
            .position(|step| {
                step.get("name").and_then(YamlValue::as_str) == Some("准备隔离服务文件配置")
            })
            .unwrap();
        let (execution, _) = command(job, &format!("cargo xtask check ci {name}"));
        assert!(configuration < execution);
    }
    let root = crate::workspace::root_dir();
    let profile: toml::Value = toml::from_str(
        &fs::read_to_string(root.join("tools/fixtures/integration/app.test.toml")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        profile["database"]["primary"]["tls_mode"].as_str(),
        Some("required")
    );
    assert_eq!(profile["redis"]["database"].as_integer(), Some(15));
    assert_eq!(profile["redis"]["mode"].as_str(), Some("required"));
}

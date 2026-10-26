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
    let (configured, settings) = steps(job)
        .iter()
        .enumerate()
        .find(|(_, step)| {
            step.get("name").and_then(YamlValue::as_str)
                == Some("写入当前 Job 的隔离数据库文件配置")
        })
        .unwrap();
    // 只提取当前配置数据，不验证写入语句或执行 Shell；健康端口以合成控制器结果渲染。
    let data = regex::Regex::new(r#"(?ms)<<['"]?EOF['"]?\r?\n(.*?)^\s*EOF\s*$"#).unwrap();
    let document = data.captures(text(settings, "run")).unwrap()[1].replace(
        "$(cat \"$RUNNER_TEMP/ryframe-full-stack/worker-health-port\")",
        "19210",
    );
    let configuration: toml::Value = toml::from_str(&document).unwrap();
    assert_eq!(configuration["api_docs"]["enabled"].as_bool(), Some(false));
    assert_eq!(
        configuration["reset"]["legacy_mysql_exclusive"].as_bool(),
        Some(true)
    );
    assert_eq!(
        configuration["object_storage"]["local_base_dir"].as_str(),
        Some("$RUNNER_TEMP/ryframe-full-stack/storage")
    );
    assert_eq!(
        configuration["reset"]["state_directory"].as_str(),
        Some("$RUNNER_TEMP/ryframe-full-stack/reset-state")
    );
    let (prepare, _) = command(job, "cargo xtask check recovery full-stack prepare");
    let (budget, _) = command(job, "cargo xtask check recovery full-stack rate-limit");
    let (start, _) = command(job, "cargo xtask check recovery full-stack start");
    let (build, _) = command(job, "corepack pnpm build");
    let (collect, collector) = command(job, "cargo xtask check recovery full-stack collect");
    assert!(
        configured < prepare
            && prepare < budget
            && budget < start
            && start < build
            && build < collect
    );
    assert_eq!(text(collector, "if"), "${{ always() }}");
    let upload = steps(job)[collect + 1..]
        .iter()
        .find(|step| uses(step, "actions/upload-artifact@"))
        .unwrap();
    assert_eq!(text(upload, "if"), "${{ always() }}");
    assert_eq!(
        text(upload.get("with").unwrap(), "if-no-files-found"),
        "error"
    );
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

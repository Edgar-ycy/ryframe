use super::*;
use std::{collections::BTreeSet, path::PathBuf};

fn recorded_configuration(
    root: &std::path::Path,
) -> (
    crate::recovery::fixture_control::source_pair::execution::tests::Fixture,
    PathBuf,
    crate::cli::FullStackConfigOptions,
) {
    use crate::{
        cli::FullStackConfigOptions,
        recovery::fixture_control::source_pair::execution::tests::Fixture,
        release::{archive::source_pair_receipt, support::write_new_json},
    };
    let source = Fixture::new();
    source.publication();
    let source_backend = source.output.join("backend");
    let source_frontend = source.output.join("frontend");
    let receipt = source_pair_receipt(&source_backend, &source_frontend, 123, 2).unwrap();
    let runtime_dir = root.join("ryframe-full-stack");
    write_new_json(&runtime_dir.join("source-pair.json"), &receipt).unwrap();
    let backend = root.join("matrix-backend");
    fs::create_dir_all(backend.join("config")).unwrap();
    fs::copy(
        crate::workspace::root_dir().join("config/app.toml"),
        backend.join("config/app.toml"),
    )
    .unwrap();
    let template = root.join("ci.toml");
    fs::copy(
        crate::workspace::root_dir().join("config/ci-full-stack.toml"),
        &template,
    )
    .unwrap();
    let options = FullStackConfigOptions {
        template,
        runtime_dir,
        source_backend,
        source_frontend,
        run_id: "123".into(),
        attempt: "2".into(),
        source_sha: receipt["backend_sha"].as_str().unwrap().into(),
        mysql_container: "mysql-job".into(),
        redis_container: "redis-job".into(),
        s3_container: "s3-job".into(),
        write: true,
        plan: false,
    };
    (source, backend, options)
}

#[test]
fn full_stack_config_cli_plan_is_readonly_and_publication_binds_job_metadata() {
    use crate::{
        ci::full_stack_config,
        cli::{CheckCommand, CiCommand, Command},
    };
    let base = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&base).unwrap();
    let temporary = tempfile::tempdir_in(base).unwrap();
    let (_sources, backend, input) = recorded_configuration(temporary.path());
    let pair_path = input.runtime_dir.join("source-pair.json");
    let pair = fs::read(&pair_path).unwrap();
    let args = [
        "check",
        "ci",
        "full-stack-config",
        "--template",
        input.template.to_str().unwrap(),
        "--runtime-dir",
        input.runtime_dir.to_str().unwrap(),
        "--run-id",
        "123",
        "--attempt",
        "2",
        "--source-sha",
        &input.source_sha,
        "--source-backend",
        input.source_backend.to_str().unwrap(),
        "--source-frontend",
        input.source_frontend.to_str().unwrap(),
        "--mysql-container",
        "mysql-job",
        "--redis-container",
        "redis-job",
        "--s3-container",
        "s3-job",
        "--plan",
    ];
    let cli = crate::cli::parse(args.into_iter().map(String::from).collect()).unwrap();
    let Command::Check(CheckCommand::Ci(CiCommand::FullStackConfig(mut options))) = cli.command
    else {
        panic!("未选择正式配置入口")
    };
    let preview = full_stack_config::plan(&options).unwrap();
    assert!(!backend.join(".local-tests").exists());
    assert_eq!(fs::read(&pair_path).unwrap(), pair);
    options.plan = false;
    options.write = true;
    let execution = full_stack_config::plan(&options).unwrap();
    assert_eq!(preview, execution);
    full_stack_config::run(&backend, &options, &execution).unwrap();
    let output = backend.join(".local-tests/full-stack-config");
    assert_eq!(
        fs::read(output.join("app.toml")).unwrap(),
        fs::read(backend.join("config/app.toml")).unwrap()
    );
    ryframe_config::AppConfig::load(&output, ryframe_config::Environment::Test).unwrap();
    let table: toml::Value =
        toml::from_str(&fs::read_to_string(output.join("app.test.toml")).unwrap()).unwrap();
    assert_eq!(table["scope_id"].as_str(), Some("ci-123-2"));
    assert_eq!(
        table["database"]["primary"]["database"].as_str(),
        Some("ryframe_e2e_123_2")
    );
    assert_eq!(
        table["reset"]["source_sha"].as_str(),
        Some(options.source_sha.as_str())
    );
    assert_eq!(
        table["reset"]["state_directory"].as_str(),
        options.runtime_dir.join("reset-state").to_str()
    );
    assert_eq!(
        table["object_storage"]["local_base_dir"].as_str(),
        options.runtime_dir.join("storage").to_str()
    );
    let port = u16::try_from(table["jobs"]["health_port"].as_integer().unwrap()).unwrap();
    assert!(std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).is_ok());
    let control: serde_json::Value = serde_json::from_slice(
        &fs::read(backend.join(".local-tests/full-stack-control.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        control,
        serde_json::json!({"format_version":1,"runtime_dir":options.runtime_dir,
        "mysql_container_id":"mysql-job","redis_container_id":"redis-job","s3_container_id":"s3-job"})
    );
    assert_eq!(fs::read(pair_path).unwrap(), pair);
    let before = fs::read(output.join("app.test.toml")).unwrap();
    assert!(full_stack_config::run(&backend, &options, &execution).is_err());
    assert_eq!(fs::read(output.join("app.test.toml")).unwrap(), before);
}

#[test]
fn full_stack_config_rejects_bad_inputs_and_preserves_private_partial_failure() {
    use crate::ci::full_stack_config;
    let base = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&base).unwrap();
    let temporary = tempfile::tempdir_in(base).unwrap();
    let (_sources, backend, options) = recorded_configuration(temporary.path());
    for field in ["run", "attempt", "sha"] {
        let mut wrong = options.clone();
        match field {
            "run" => wrong.run_id = "124".into(),
            "attempt" => wrong.attempt = "3".into(),
            _ => {
                let first = if wrong.source_sha.starts_with('a') {
                    "b"
                } else {
                    "a"
                };
                wrong.source_sha.replace_range(0..1, first);
            }
        }
        let plan = full_stack_config::plan(&wrong).unwrap();
        assert!(full_stack_config::run(&backend, &wrong, &plan).is_err());
        assert!(!backend.join(".local-tests").exists());
    }
    let mut overflow = options.clone();
    overflow.run_id = "18446744073709551616".into();
    assert!(full_stack_config::plan(&overflow).is_err());
    let plan = full_stack_config::plan(&options).unwrap();
    let extra = options.runtime_dir.join("unknown");
    fs::write(&extra, b"preserved").unwrap();
    assert!(full_stack_config::run(&backend, &options, &plan).is_err());
    assert_eq!(fs::read(&extra).unwrap(), b"preserved");
    fs::remove_file(extra).unwrap();
    let source = options.source_backend.join("src/lib.rs");
    let original = fs::read(&source).unwrap();
    fs::write(&source, b"dirty source").unwrap();
    assert!(full_stack_config::run(&backend, &options, &plan).is_err());
    fs::write(&source, original).unwrap();
    let pair_path = options.runtime_dir.join("source-pair.json");
    let pair = fs::read(&pair_path).unwrap();
    assert!(!backend.join(".local-tests").exists());
    #[cfg(unix)]
    {
        let link = temporary.path().join("linked.toml");
        std::os::unix::fs::symlink(&options.template, &link).unwrap();
        let mut linked = options.clone();
        linked.template = link;
        assert!(full_stack_config::run(&backend, &linked, &plan).is_err());
        assert!(!backend.join(".local-tests").exists());
        let saved = temporary.path().join("original-pair.json");
        fs::rename(&pair_path, &saved).unwrap();
        assert!(full_stack_config::run(&backend, &options, &plan).is_err());
        std::os::unix::fs::symlink(&saved, &pair_path).unwrap();
        assert!(full_stack_config::run(&backend, &options, &plan).is_err());
        assert_eq!(fs::read(&saved).unwrap(), pair);
        fs::remove_file(&pair_path).unwrap();
        fs::rename(saved, &pair_path).unwrap();
    }
    let mut document: toml::Value =
        toml::from_str(&fs::read_to_string(&options.template).unwrap()).unwrap();
    document["auth"]["jwt_secret"] = " \t ".into();
    document["object_storage"]["secret_key"] = "private-marker".into();
    fs::write(&options.template, toml::to_string(&document).unwrap()).unwrap();
    let error = full_stack_config::run(&backend, &options, &plan)
        .unwrap_err()
        .to_string();
    assert!(error.contains("完整应用配置校验失败") && error.contains("保留局部现场"));
    assert!(error.contains("auth.jwt_secret"));
    assert!(!error.contains("private-marker"));
    let output = backend.join(".local-tests/full-stack-config");
    assert_eq!(
        fs::read(output.join("app.toml")).unwrap(),
        fs::read(backend.join("config/app.toml")).unwrap()
    );
    assert!(output.join("app.test.toml").is_file());
    assert!(
        !backend
            .join(".local-tests/full-stack-control.json")
            .exists()
    );
    let bytes = fs::read(output.join("app.test.toml")).unwrap();
    assert!(full_stack_config::run(&backend, &options, &plan).is_err());
    assert_eq!(fs::read(output.join("app.test.toml")).unwrap(), bytes);
    assert_eq!(fs::read(pair_path).unwrap(), pair);
}
#[test]
fn extended_full_stack_uses_file_configuration_and_only_app_env() {
    let extended = workflow("extended-ci.yml");
    let job = job(&extended, "full-stack-e2e");
    let environment = job.get("env").unwrap();
    assert_eq!(text(environment, "APP_ENV"), "test");
    let application_keys = environment
        .as_mapping()
        .unwrap()
        .iter()
        .map(|(name, _)| match name {
            yaml::YamlKey::String(name) => name.as_str(),
            _ => panic!("工作流环境键必须是字符串"),
        })
        .filter(|name| name.starts_with("APP_"))
        .collect::<Vec<_>>();
    assert_eq!(application_keys, ["APP_ENV"]);

    let (_, configuration) = fixture_configuration(job);
    assert_eq!(configuration["snowflake_worker_id"].as_integer(), Some(3));
    assert_eq!(configuration["app"]["host"].as_str(), Some("127.0.0.1"));
    assert_eq!(configuration["app"]["port"].as_integer(), Some(8080));
    assert_eq!(configuration["jobs"]["mode"].as_str(), Some("external"));
    assert_eq!(
        configuration["jobs"]["health_host"].as_str(),
        Some("127.0.0.1")
    );

    assert_eq!(
        configuration["multi_tenancy"]["enabled"].as_bool(),
        Some(true)
    );
    assert_eq!(configuration["redis"]["database"].as_integer(), Some(14));
    assert_eq!(
        configuration["object_storage"]["backend"].as_str(),
        Some("s3")
    );
    assert!(configuration["auth"]["jwt_secret"].as_str().unwrap().len() >= 32);
    assert!(
        !configuration["bootstrap"]["admin_password"]
            .as_str()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        configuration["rate_limit"]["api_limits"]["POST /api/v1/auth/login"].as_integer(),
        Some(5)
    );
}

#[test]
fn diagnostic_upload_excludes_private_files_but_keeps_public_api_log() {
    let extended = workflow("extended-ci.yml");
    let upload = steps(job(&extended, "full-stack-e2e"))
        .iter()
        .find(|step| step.get("name").and_then(YamlValue::as_str) == Some("保存全栈诊断产物"))
        .unwrap();
    let settings = upload.get("with").unwrap();
    let patterns = text(settings, "path");
    let parent = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&parent).unwrap();
    let root = tempfile::Builder::new()
        .prefix("ci-artifact-")
        .tempdir_in(parent)
        .unwrap();
    let sensitive = [
        "frontend/.local-tests/frontend-tools.json",
        "fixtures/business/frontend/.local-tests/frontend-tools.json",
        "runner/ryframe-full-stack/config/app.test.toml",
        "runner/ryframe-full-stack/worker-config/app.test.toml",
        "backend/.local-tests/full-stack-config/app.test.toml",
    ]
    .map(|path| root.path().join(path));
    let public = root.path().join("runner/ryframe-full-stack/api.log");
    for path in sensitive.iter().chain(std::iter::once(&public)) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "fixture").unwrap();
    }
    let runner = root
        .path()
        .join("runner")
        .to_string_lossy()
        .replace('\\', "/");
    let resolve = |pattern: &str| {
        root.path()
            .join(pattern.replace("${{ runner.temp }}", &runner))
            .to_string_lossy()
            .replace('\\', "/")
    };
    let options = glob::MatchOptions {
        case_sensitive: true,
        require_literal_separator: true,
        require_literal_leading_dot: false,
    };
    // 现行工作流仅用 !、** 和目录输入；不模拟整个 Actions 上传、链接或权限处理。
    let excluded = patterns
        .lines()
        .filter_map(|line| line.trim().strip_prefix('!'))
        .flat_map(|pattern| {
            let selected = resolve(pattern);
            // Actions 的目录/** 排除包含文件；glob 0.3.4 尾部 ** 只枚举目录。
            // 仅适配现行目录树排除，追加 * 让库枚举真实文件，不扩展为通用 Actions matcher。
            let selected = if selected.ends_with("/**") {
                format!("{selected}/*")
            } else {
                selected
            };
            glob::glob_with(&selected, options)
                .unwrap()
                .map(Result::unwrap)
        })
        .filter(|path| path.is_file())
        .collect::<BTreeSet<PathBuf>>();
    for path in sensitive {
        assert!(
            excluded.contains(&path),
            "敏感文件未被实际上传排除模式覆盖：{}",
            path.display()
        );
    }
    assert!(!excluded.contains(&public), "公开 API 日志不得被排除");
    assert!(
        patterns
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('!'))
            .any(|pattern| {
                let selected = resolve(pattern);
                // upload-artifact 的目录输入会递归包含目录内文件。
                (std::path::Path::new(&selected).is_dir() && public.starts_with(&selected))
                    || glob::glob_with(&selected, options)
                        .unwrap()
                        .any(|path| path.unwrap() == public)
            }),
        "公开 API 日志必须仍由当前正向上传路径包含"
    );
}

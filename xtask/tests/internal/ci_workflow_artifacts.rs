use super::*;
use std::{collections::BTreeSet, path::PathBuf};

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
        configuration["jobs"]["health_port"].as_integer(),
        Some(19210)
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
    assert_eq!(
        settings.get("include-hidden-files"),
        Some(&YamlValue::Bool(true))
    );
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

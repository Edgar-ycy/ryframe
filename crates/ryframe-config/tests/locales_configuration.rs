use std::path::Path;

use ryframe_config::{AppConfig, AppSettings};
use ryframe_kernel::AppError;

fn settings(directory: Option<&str>) -> AppSettings {
    let mut table: toml::Table =
        toml::from_str("name = \"locale-test\"\nhost = \"127.0.0.1\"\nport = 8080\n")
            .expect("基础应用字段必须有效");
    if let Some(directory) = directory {
        table.insert("locales_dir".into(), toml::Value::String(directory.into()));
    }
    table.try_into().expect("路径字符串必须可以反序列化")
}

fn configuration() -> AppConfig {
    let mut config: AppConfig = toml::from_str(include_str!("../../../config/app.toml"))
        .expect("默认配置必须可以离线反序列化");
    // 路径测试不建立 Redis，也不依赖加载器为其注入作用域。
    config.redis = None;
    config.validate().expect("路径测试的基础配置必须有效");
    config
}

#[test]
fn omitted_directory_and_default_settings_both_use_locales() {
    assert_eq!(settings(None).locales_dir, Path::new("locales"));
    assert_eq!(AppSettings::default().locales_dir, Path::new("locales"));
}

#[test]
fn checked_in_configuration_declares_the_directory_explicitly() {
    let config: toml::Value =
        toml::from_str(include_str!("../../../config/app.toml")).expect("默认配置必须是有效 TOML");
    assert_eq!(config["app"]["locales_dir"].as_str(), Some("locales"));
}

#[test]
fn explicit_paths_preserve_unicode_spaces_and_literal_templates() {
    for directory in [
        " 本地 语言资源/词典 ",
        " ",
        "./自定义 locales",
        "../共享 语言资源",
        "${APP_ENV}/字典",
        "%APP_ENV%/字典",
    ] {
        let parsed = settings(Some(directory));
        assert_eq!(
            parsed.locales_dir.as_os_str(),
            Path::new(directory).as_os_str()
        );
        assert!(parsed.locales_dir.is_relative());
        let mut config = configuration();
        config.app = parsed;
        config
            .validate()
            .expect("有效路径不得被修剪、展开或访问文件系统");
        assert_eq!(
            config.app.locales_dir.as_os_str(),
            Path::new(directory).as_os_str()
        );
    }
}

#[test]
fn empty_and_control_character_paths_fail_without_echoing_content() {
    for directory in [
        "",
        "locale-secret\0resource",
        "locale-secret\rresource",
        "locale-secret\nresource",
        "locale-secret\r\nresource",
    ] {
        let mut config = configuration();
        config.app = settings(Some(directory));
        let error = config.validate().expect_err("空路径和控制字符必须失败");
        let AppError::Config(reason) = error else {
            panic!("必须返回配置错误");
        };
        assert_eq!(
            reason,
            "app.locales_dir 必须是非空且不含 NUL、CR 或 LF 的路径；请修正配置文件"
        );
        assert!(!reason.contains("locale-secret"));
    }
}

#[test]
fn directory_rejects_non_string_configuration_values() {
    for invalid in [
        toml::Value::Integer(1),
        toml::Value::Boolean(true),
        toml::Value::Array(vec![toml::Value::String("locales".into())]),
    ] {
        let mut table: toml::Table =
            toml::from_str("name = \"locale-test\"\nhost = \"127.0.0.1\"\nport = 8080\n").unwrap();
        table.insert("locales_dir".into(), invalid);
        let result: Result<AppSettings, _> = table.try_into();
        assert!(result.is_err());
    }
}

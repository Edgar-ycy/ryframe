use std::io::Write;
use std::sync::Mutex;

/// i18n 模块测试
/// 从 crates/ryframe-common/src/i18n.rs 内联测试迁移
use ryframe_common::i18n::{I18nManager, detect_language};

/// 互斥锁：防止 cargo test 并行运行时多个测试同时操作同一临时目录
static TEST_DIR_MUTEX: Mutex<()> = Mutex::new(());

fn create_test_locale_dir() -> std::path::PathBuf {
    // 串行化目录创建，避免并发删除/创建导致的竞态条件
    let _guard = TEST_DIR_MUTEX.lock().unwrap();

    let dir = std::env::temp_dir().join("ryframe_i18n_test");
    let _ = std::fs::create_dir_all(&dir);

    // File::create 使用截断模式，会覆盖已存在的文件
    let zh_cn = dir.join("zh-CN.toml");
    let mut f = std::fs::File::create(&zh_cn).unwrap();
    writeln!(f, r#"[common]"#).unwrap();
    writeln!(f, r#"success = "操作成功""#).unwrap();
    writeln!(f, r#"fail = "操作失败""#).unwrap();
    writeln!(f).unwrap();
    writeln!(f, r#"[error]"#).unwrap();
    writeln!(f, r#"not_found = "资源不存在""#).unwrap();
    writeln!(f).unwrap();
    writeln!(f, r#"[user]"#).unwrap();
    writeln!(f, r#"welcome = "欢迎 {{name}}""#).unwrap();
    f.flush().unwrap();
    drop(f);

    let en_us = dir.join("en-US.toml");
    let mut f = std::fs::File::create(&en_us).unwrap();
    writeln!(f, r#"[common]"#).unwrap();
    writeln!(f, r#"success = "Operation successful""#).unwrap();
    writeln!(f, r#"fail = "Operation failed""#).unwrap();
    writeln!(f).unwrap();
    writeln!(f, r#"[error]"#).unwrap();
    writeln!(f, r#"not_found = "Resource not found""#).unwrap();
    writeln!(f).unwrap();
    writeln!(f, r#"[user]"#).unwrap();
    writeln!(f, r#"welcome = "Welcome {{name}}""#).unwrap();
    f.flush().unwrap();
    drop(f);

    dir
}

#[test]
fn test_load_and_translate() {
    let dir = create_test_locale_dir();
    let i18n = I18nManager::load(&dir).unwrap();

    assert_eq!(i18n.translate("common.success", "zh-CN"), "操作成功");
    assert_eq!(
        i18n.translate("common.success", "en-US"),
        "Operation successful"
    );
    assert_eq!(i18n.translate("error.not_found", "zh-CN"), "资源不存在");
}

#[test]
fn test_fallback_to_default() {
    let dir = create_test_locale_dir();
    let i18n = I18nManager::load(&dir).unwrap();

    // ja-JP 不存在 → 回退到默认 zh-CN
    assert_eq!(i18n.translate("common.success", "ja-JP"), "操作成功");
}

#[test]
fn test_missing_key_returns_key() {
    let dir = create_test_locale_dir();
    let i18n = I18nManager::load(&dir).unwrap();

    // 键不存在 → 返回键本身
    assert_eq!(
        i18n.translate("nonexistent.key", "zh-CN"),
        "nonexistent.key"
    );
}

#[test]
fn test_translate_with_args() {
    let dir = create_test_locale_dir();
    let i18n = I18nManager::load(&dir).unwrap();

    assert_eq!(
        i18n.translate_with_args("user.welcome", "zh-CN", &[("name", "张三")]),
        "欢迎 张三"
    );
    assert_eq!(
        i18n.translate_with_args("user.welcome", "en-US", &[("name", "Alice")]),
        "Welcome Alice"
    );
}

#[test]
fn test_detect_language_exact_match() {
    let supported = vec!["zh-CN".into(), "en-US".into(), "ja-JP".into()];
    assert_eq!(detect_language(Some("en-US"), &supported), "en-us");
    assert_eq!(detect_language(Some("ja-JP"), &supported), "ja-jp");
}

#[test]
fn test_detect_language_prefix_match() {
    let supported = vec!["zh-CN".into(), "en-US".into()];
    // "en" 前缀匹配 "en-US"
    assert_eq!(detect_language(Some("en"), &supported), "en-US");
    // "zh" 前缀匹配 "zh-CN"
    assert_eq!(detect_language(Some("zh"), &supported), "zh-CN");
}

#[test]
fn test_detect_language_q_value() {
    let supported = vec!["zh-CN".into(), "en-US".into()];
    // zh-CN;q=0.9,en-US;q=0.8 → 应该选 q 值更高的 zh-cn
    let result = detect_language(Some("zh-CN;q=0.9,en-US;q=0.8"), &supported);
    assert_eq!(result, "zh-cn");
}

#[test]
fn test_detect_language_none_header() {
    let supported = vec!["zh-CN".into(), "en-US".into()];
    assert_eq!(detect_language(None, &supported), "zh-CN");
}

#[test]
fn test_detect_language_empty_supported() {
    let supported: Vec<String> = vec![];
    assert_eq!(detect_language(Some("en-US"), &supported), "zh-CN");
}

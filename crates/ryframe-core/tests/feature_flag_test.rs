/// feature_flag 模块测试
/// 从 crates/ryframe-core/src/feature_flag.rs 内联测试迁移
use ryframe_core::feature_flag::{FeatureFlags, FeaturePresets};

#[test]
fn test_basic_flag() {
    let flags = FeatureFlags::new()
        .with_flag("feature_a", true, "功能A")
        .with_flag("feature_b", false, "功能B");

    assert!(flags.is_enabled("feature_a"));
    assert!(!flags.is_enabled("feature_b"));
    assert!(!flags.is_enabled("unknown"));
}

#[test]
fn test_is_enabled_or() {
    let flags = FeatureFlags::new();
    assert!(!flags.is_enabled("unknown"));
    assert!(flags.is_enabled_or("unknown", true));
    assert!(!flags.is_enabled_or("unknown", false));
}

#[test]
fn test_set_enabled() {
    let flags = FeatureFlags::new().with_flag("test_flag", false, "测试");

    assert!(!flags.is_enabled("test_flag"));
    assert!(flags.set_enabled("test_flag", true));
    assert!(flags.is_enabled("test_flag"));
    assert!(flags.set_enabled("test_flag", false));
    assert!(!flags.is_enabled("test_flag"));

    assert!(!flags.set_enabled("nonexistent", true));
}

#[test]
fn test_toggle() {
    let flags = FeatureFlags::new().with_flag("toggle_test", false, "Toggle");

    assert_eq!(flags.toggle("toggle_test"), Some(true));
    assert!(flags.is_enabled("toggle_test"));
    assert_eq!(flags.toggle("toggle_test"), Some(false));
    assert!(!flags.is_enabled("toggle_test"));

    assert_eq!(flags.toggle("nonexistent"), None);
}

#[test]
fn test_system_flag_immutable() {
    let flags = FeatureFlags::new().with_system_flag("core_auth", true, "核心认证");

    assert!(flags.is_enabled("core_auth"));
    assert!(!flags.set_enabled("core_auth", false));
    assert!(flags.is_enabled("core_auth"));
    assert_eq!(flags.toggle("core_auth"), None);
}

#[test]
fn test_list_all() {
    let flags = FeatureFlags::new()
        .with_flag("a", true, "A")
        .with_flag("b", false, "B");

    let all = flags.list_all();
    assert_eq!(all.len(), 2);
}

#[test]
fn test_enabled_flags() {
    let flags = FeatureFlags::new()
        .with_flag("a", true, "A")
        .with_flag("b", false, "B")
        .with_system_flag("core", true, "核心");

    let enabled = flags.enabled_flags();
    assert_eq!(enabled.len(), 1);
    assert_eq!(enabled[0].key, "a");
}

#[test]
fn test_export_import() {
    let flags = FeatureFlags::new()
        .with_flag("a", true, "A")
        .with_flag("b", false, "B");

    let config = flags.export_config();
    assert_eq!(config.get("a"), Some(&true));
    assert_eq!(config.get("b"), Some(&false));

    let flags2 = FeatureFlags::from_config(&config);
    assert!(flags2.is_enabled("a"));
    assert!(!flags2.is_enabled("b"));
}

#[test]
fn test_presets() {
    let flags = FeaturePresets::standard();
    assert!(flags.is_enabled("user_registration"));
    assert!(!flags.is_enabled("beta_features"));

    let dev_flags = FeaturePresets::development();
    assert!(!dev_flags.is_enabled("email_verification"));
    assert!(dev_flags.is_enabled("beta_features"));
}

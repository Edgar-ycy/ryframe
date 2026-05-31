/// email 模块测试
/// 从 crates/ryframe-common/src/utils/email.rs 内联测试迁移
use ryframe_common::utils::email::{EmailConfig, EmailSender};

#[test]
fn test_email_config_default() {
    let config = EmailConfig::default();
    assert_eq!(config.smtp_port, 587);
    assert_eq!(config.from_name, "System");
    assert!(config.enable_tls);
}

#[test]
fn test_build_message() {
    let config = EmailConfig {
        smtp_host: "smtp.test.com".into(),
        smtp_port: 587,
        smtp_username: "user".into(),
        smtp_password: "pass".into(),
        from_name: "Test".into(),
        from_email: "test@test.com".into(),
        enable_tls: false,
    };

    let sender = EmailSender::new(config);
    let msg = sender.build_message("to@test.com", "Subject", "<h1>Hello</h1>");
    assert!(msg.is_ok());
}

#[test]
fn test_build_message_invalid_email() {
    let sender = EmailSender::new(EmailConfig::default());
    let msg = sender.build_message("invalid-email", "Subject", "Body");
    assert!(msg.is_err());
}

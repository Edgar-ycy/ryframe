use ryframe_auth::password;

use super::{identity::seed_credentials, operations::verify_seed_account};

const ADMIN_PASSWORD: &str = "Reset!Admin123";

#[test]
fn ordinary_seed_uses_initial_password_and_both_accounts_require_password_change() {
    let seeds = seed_credentials(Some(ADMIN_PASSWORD)).expect("管理员密码符合复杂度要求");
    assert!(password::verify(password::INITIAL_USER_PASSWORD, &seeds.user_hash).unwrap());
    assert!(password::verify(ADMIN_PASSWORD, &seeds.admin_hash).unwrap());
    assert!(!password::verify(ADMIN_PASSWORD, &seeds.user_hash).unwrap());
    assert!(!password::verify(password::INITIAL_USER_PASSWORD, &seeds.admin_hash).unwrap());

    for (username, hash) in [("admin", &seeds.admin_hash), ("user", &seeds.user_hash)] {
        assert!(verify_seed_account(username, hash, 1, &seeds).is_ok());
        for flag in [0, -1, 2] {
            assert!(verify_seed_account(username, hash, flag, &seeds).is_err());
        }
    }
}

#[test]
fn administrator_credentials_fail_closed_without_exposing_passwords() {
    for (value, reason) in [
        (None, "缺少"),
        (Some("不可解码凭据"), "复杂度"),
        (Some(""), "复杂度"),
        (Some(password::INITIAL_USER_PASSWORD), "复杂度"),
        (Some("Onlyletters"), "复杂度"),
    ] {
        let error = match seed_credentials(value) {
            Ok(_) => panic!("无效管理员凭据不得通过预检"),
            Err(error) => error.to_string(),
        };
        assert!(!error.contains(password::INITIAL_USER_PASSWORD));
        assert!(!error.contains("Onlyletters"));
        assert!(!error.contains("不可解码凭据"));
        for detail in [
            reason,
            "bootstrap.admin_password",
            "所选配置文件",
            "8~72",
            "可见 ASCII",
            "大小写字母",
            "数字",
            "符号",
        ] {
            assert!(error.contains(detail), "管理员诊断缺少原因或修复指引");
        }
    }
}

#[test]
fn persisted_password_mismatches_and_invalid_hashes_fail_closed() {
    let seeds = seed_credentials(Some(ADMIN_PASSWORD)).expect("管理员密码符合复杂度要求");
    assert!(verify_seed_account("admin", &seeds.user_hash, 1, &seeds).is_err());
    assert!(verify_seed_account("user", &seeds.admin_hash, 1, &seeds).is_err());
    for username in ["admin", "user", "unknown"] {
        let error = verify_seed_account(username, "invalid-hash", 1, &seeds)
            .unwrap_err()
            .to_string();
        assert!(!error.contains(ADMIN_PASSWORD));
        assert!(!error.contains("invalid-hash"));
    }
}

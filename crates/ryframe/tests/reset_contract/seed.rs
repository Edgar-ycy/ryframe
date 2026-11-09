use ryframe_auth::password;

use super::{
    RECREATE_SCHEMA_PRIVILEGES, REQUIRED_SCHEMA_PRIVILEGES,
    identity::seed_credentials,
    operations::{has_database_privileges, verify_seed_account},
};

const ADMIN_PASSWORD: &str = "Reset!Admin123";

#[test]
fn rebuild_requires_privileges_for_the_exact_schema() {
    let permits = |grants: &[&str], case_sensitive, literal_names| {
        let grants = grants
            .iter()
            .map(|grant| (*grant).to_owned())
            .collect::<Vec<_>>();
        has_database_privileges(
            &grants,
            "fixture_seed",
            case_sensitive,
            literal_names,
            REQUIRED_SCHEMA_PRIVILEGES,
        )
    };
    let exact = r"GRANT ALL PRIVILEGES ON `fixture\_seed`.* TO `account`@`host`";
    let literal = "GRANT ALL PRIVILEGES ON `fixture_seed`.* TO `account`@`host`";
    let upper = r"GRANT ALL PRIVILEGES ON `FIXTURE\_SEED`.* TO `account`@`host`";
    let global = "GRANT ALL PRIVILEGES ON *.* TO `account`@`host`";
    assert!(permits(&[exact], true, false));
    assert!(permits(&[literal], true, true));
    assert!(permits(&[upper], false, false));
    assert!(permits(&[global], true, true));
    assert!(!permits(&[literal], true, false));
    assert!(!permits(&[upper], true, false));
    for grant in [
        r"GRANT ALL PRIVILEGES ON `fixture\_other`.* TO `account`@`host`",
        "GRANT ALL PRIVILEGES ON `fixture%`.* TO `account`@`host`",
        "GRANT ALL PRIVILEGES ON `fixture_seed`.`table` TO `account`@`host`",
        r"GRANT SELECT, CREATE, DROP ON `fixture\_seed`.* TO `account`@`host`",
    ] {
        assert!(!permits(&[grant], true, false));
    }
    let revoke = "REVOKE DROP ON `fixture_seed`.* FROM `account`@`host`";
    assert!(!permits(&[global, revoke], true, true));
}

#[test]
fn rebuild_lock_permissions_do_not_follow_another_connections_grant() {
    let holding = vec![r"GRANT SELECT, CREATE, DROP ON `fixture\_first`.* TO `lock`@`host`".into()];
    let target = vec![r"GRANT ALL PRIVILEGES ON `fixture\_second`.* TO `target`@`host`".into()];
    assert!(has_database_privileges(
        &target,
        "fixture_second",
        true,
        false,
        REQUIRED_SCHEMA_PRIVILEGES,
    ));
    assert!(!has_database_privileges(
        &holding,
        "fixture_second",
        true,
        false,
        RECREATE_SCHEMA_PRIVILEGES,
    ));
    assert!(has_database_privileges(
        &holding,
        "fixture_first",
        true,
        false,
        RECREATE_SCHEMA_PRIVILEGES,
    ));
}

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

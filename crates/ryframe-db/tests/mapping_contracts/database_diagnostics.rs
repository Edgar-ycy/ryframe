use ryframe_config::DbConnection;
use ryframe_db::connection::redact_database_diagnostic;

#[test]
fn known_credentials_are_masked_before_diagnostic_whitespace_is_normalized() {
    const PASSWORD: &str = "  p'\\ass@word  ";
    let config = DbConnection {
        username: "private-user".into(),
        password: PASSWORD.into(),
        tls_client_key: Some("  private-key.pem  ".into()),
        ..DbConnection::default()
    };
    let escaped = format!("{PASSWORD:?}");
    let sql_escaped = PASSWORD.replace('\\', "\\\\").replace('\'', "\\'");
    let sql_quoted = PASSWORD.replace('\'', "''");
    for message in [
        format!(" \t1045  connect failed [{PASSWORD}]\r\nretry "),
        "1045 connect failed [p'\\ass@word] retry".into(),
        format!("1045 connect failed {escaped} retry"),
        format!("1045 connect failed {sql_escaped} retry"),
        format!("1045 connect failed {sql_quoted} retry"),
        "1045 connect failed %20%20p%27%5Cass%40word%20%20 private%2Duser %20%20private%2Dkey%2Epem%20%20 retry"
            .into(),
        "1045 connect failed private-user [  private-key.pem  ] retry".into(),
    ] {
        let diagnostic = redact_database_diagnostic([&config], [], &message);
        assert!(diagnostic.contains("1045"));
        assert!(diagnostic.contains("connect failed"));
        assert!(diagnostic.contains("retry"));
        for secret in [
            PASSWORD,
            r"p'\ass@word",
            r"p'\\ass@word",
            r"p\'\\ass@word",
            r"p''\ass@word",
            "p%27%5Cass%40word",
            "private-user",
            "private%2Duser",
            "private-key.pem",
            "private%2Dkey%2Epem",
        ] {
            assert!(!diagnostic.contains(secret), "诊断不得残留凭据");
        }
    }
}

#[test]
fn overlapping_secrets_and_uri_tokens_are_masked_from_the_same_original_message() {
    let cases: &[(&[&str], &str, &str)] = &[
        (
            &["abcdef", "defXYZ"],
            "1045 abcdefXYZ ready",
            "1045 <已隐藏> ready",
        ),
        (&["aba"], "1045 ababa ready", "1045 <已隐藏> ready"),
        (
            &["密钥α", "α凭据"],
            "1045 密钥α凭据 ready",
            "1045 <已隐藏> ready",
        ),
        (
            &["prefix mysql://user:pw@host/db tail"],
            "1045 keep prefix mysql://user:pw@host/db tail ready",
            "1045 keep <连接地址已隐藏> ready",
        ),
        (
            &["  alpha\tbeta  "],
            "1045 alpha beta ready",
            "1045 <已隐藏> ready",
        ),
        (
            &[],
            "1045 \t(mysql://unlisted:pw@host/db)\u{2003} ready",
            "1045 <连接地址已隐藏> ready",
        ),
        (
            &[],
            "1045 \t handshake failed\r\n ready",
            "1045 handshake failed ready",
        ),
        (&[""], "1045 ready", "1045 ready"),
    ];
    for (secrets, message, expected) in cases {
        assert_eq!(
            redact_database_diagnostic([], secrets.iter().copied(), message),
            *expected
        );
    }
}

#[test]
fn private_key_material_stays_hidden_when_known_secrets_overlap_its_marker() {
    let cases: &[(&[&str], &str)] = &[
        (&[], "1045 -----BEGIN PRIVATE KEY----- private-material"),
        (
            &["-----BEGIN"],
            "1045 -----BEGIN RSA PRIVATE KEY----- private-material",
        ),
        (
            &["-----BEGIN", "KEY-----"],
            "1045 -----BEGIN EC PRIVATE\tKEY----- private-material",
        ),
    ];
    for (secrets, message) in cases {
        assert_eq!(
            redact_database_diagnostic([], secrets.iter().copied(), message),
            "数据库诊断包含私钥材料，原文已隐藏；请核对所选配置的 TLS 字段及私钥格式"
        );
    }
}

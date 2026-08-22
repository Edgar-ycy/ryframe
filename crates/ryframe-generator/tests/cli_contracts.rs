use ryframe_generator::cli::{CliEnvironment, CliOptions, SecretDatabaseUrl, help_text, parse_cli};

fn environment() -> CliEnvironment {
    CliEnvironment {
        database_url: Some(
            "mysql://env_user:env_secret@db.internal:3306/tenant_catalog".to_owned(),
        ),
        tables: Some("biz_device,biz_work_order".to_owned()),
    }
}

fn parsed(arguments: &[&str]) -> CliOptions {
    parse_cli(
        arguments.iter().map(|argument| (*argument).to_owned()),
        environment(),
    )
    .expect("参数应通过校验")
    .expect("不应进入帮助分支")
}

fn parse_error(arguments: &[&str]) -> String {
    match parse_cli(
        arguments.iter().map(|argument| (*argument).to_owned()),
        environment(),
    ) {
        Err(error) => error.to_string(),
        Ok(_) => panic!("参数应被拒绝"),
    }
}

#[test]
fn environment_input_defaults_to_dry_run_without_overwrite() {
    let options = parsed(&[]);

    assert!(!options.write);
    assert!(!options.generate.overwrite);
    assert_eq!(
        options.generate.repository_dir,
        "crates/ryframe-db/src/repositories/business"
    );
    assert_eq!(
        options.generate.use_case_dir,
        "crates/ryframe-application/src/business"
    );
    assert_eq!(options.generate.tables, ["biz_device", "biz_work_order"]);
}

#[test]
fn command_line_overrides_environment_and_requires_explicit_write_flags() {
    let options = parsed(&[
        "--database-url",
        "mysql://cli_user:cli_secret@127.0.0.1:3307/cli_catalog?ssl-mode=required",
        "--tables",
        "biz_alpha,biz_beta",
        "--repository-dir",
        "crates/ryframe-db/src/repositories/generated",
        "--use-case-dir",
        "crates/ryframe-application/src/business/generated",
        "--write",
        "--overwrite",
    ]);

    assert!(options.write);
    assert!(options.generate.overwrite);
    assert_eq!(
        options.database_url.target(),
        "mysql://127.0.0.1:3307/cli_catalog"
    );
    assert_eq!(options.generate.tables, ["biz_alpha", "biz_beta"]);
    assert_eq!(
        options.generate.repository_dir,
        "crates/ryframe-db/src/repositories/generated"
    );
}

#[test]
fn overwrite_without_write_is_rejected() {
    let error = parse_error(&["--overwrite"]);
    assert!(error.contains("--write"));
}

#[test]
fn output_paths_reject_parent_and_absolute_paths() {
    for (flag, path) in [
        ("--entity-dir", "../outside"),
        ("--repository-dir", "C:\\outside"),
        ("--use-case-dir", "/tmp/outside"),
        ("--handler-dir", "crates/ryframe-api/../outside"),
    ] {
        let error = parse_error(&[flag, path]);
        assert!(
            error.contains("工作区相对路径") || error.contains("非法路径片段"),
            "{flag} 应拒绝 {path}"
        );
    }
}

#[test]
fn database_target_and_debug_output_never_expose_credentials() {
    let url = SecretDatabaseUrl::new(
        "mysql://admin:p%40ssword@db.example.com:3306/ryframe?password=hidden#secret".to_owned(),
    )
    .expect("数据库 URL 应有效");
    let rendered = format!("{} {url:?}", url.target());

    assert_eq!(url.target(), "mysql://db.example.com:3306/ryframe");
    for secret in ["admin", "p%40ssword", "hidden", "secret"] {
        assert!(!rendered.contains(secret));
    }
}

#[test]
fn invalid_database_url_error_does_not_echo_secret() {
    let secret = "postgres://admin:do-not-print@db.example.com/catalog";
    let error = SecretDatabaseUrl::new(secret.to_owned())
        .expect_err("非 MySQL URL 应被拒绝")
        .to_string();

    assert!(!error.contains("admin"));
    assert!(!error.contains("do-not-print"));
}

#[test]
fn help_does_not_require_database_configuration() {
    let result =
        parse_cli(["--help".to_owned()], CliEnvironment::default()).expect("帮助参数应成功");
    assert!(result.is_none());
    assert!(help_text().contains("默认                         dry-run"));
}

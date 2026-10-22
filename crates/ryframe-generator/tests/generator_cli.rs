use std::{fs, process::Command};

fn run(root: &std::path::Path, arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ryframe-generate"))
        .args(arguments)
        .current_dir(root)
        .env("PATH", "")
        .output()
        .unwrap()
}

#[test]
fn plans_do_not_spawn_cargo_or_read_database_configuration() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("Cargo.toml"),
        "[workspace]\nmembers = []\n",
    )
    .unwrap();
    fs::create_dir(root.path().join("config")).unwrap();
    fs::write(
        root.path().join("config/tools.toml"),
        "[generator]\npackage = 'order-business'\nsource = 'legacy'\nstorage = 'control'\ntables = ['biz_order', 'biz_order_item']\n",
    )
    .unwrap();
    fs::write(
        root.path().join("config/app.toml"),
        "这不是有效的运行时配置",
    )
    .unwrap();
    for args in [
        vec!["new-business", "order-business", "--plan"],
        vec!["resource", "--plan"],
        vec!["import", "--table", "biz_order", "--plan"],
        vec!["import", "--tables", "biz_order,biz_order_item", "--plan"],
        vec!["resource", "--models", "Order,OrderItem", "--plan"],
        vec!["run", "--plan"],
        vec![
            "--config-dir",
            "不存在 配置",
            "import",
            "--tables",
            "table1,table2",
            "--plan",
        ],
    ] {
        let output = run(root.path(), &args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("计划"));
        assert!(!root.path().join("target").exists());
        assert!(!root.path().join("crates").exists());
        assert!(!root.path().join("Cargo.lock").exists());
        assert!(!root.path().join(".local-tests").exists());
    }
}

#[test]
fn ambiguous_arguments_fail_before_any_task() {
    let root = tempfile::tempdir().unwrap();
    for args in [
        vec!["resource", "--package", "--write"],
        vec!["resource", "--package", "one", "--package", "two"],
        vec!["new-business", "order", "--write", "--plan"],
        vec!["new-business", "order", "--write", "--write"],
        vec!["import", "--table", "--write"],
        vec!["import", "--table", "a", "--tables", "b"],
        vec![
            "--config-dir",
            "first",
            "resource",
            "--config-dir",
            "second",
        ],
        vec!["resource", "--config-dir"],
        vec!["resource", "--config-dir="],
        vec!["resource", "--config-dir", ""],
        vec!["resource", "--", "--config-dir", "ignored"],
    ] {
        assert_eq!(run(root.path(), &args).status.code(), Some(2));
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

#[cfg(feature = "schema-import")]
#[test]
fn import_from_business_subdirectory_uses_workspace_config_and_honors_explicit_override() {
    let root = tempfile::tempdir().unwrap();
    let business = root.path().join("crates/orders-business");
    fs::create_dir_all(business.join("src")).unwrap();
    fs::write(
        root.path().join("Cargo.toml"),
        "[workspace]\nmembers=['crates/orders-business']\nresolver='3'\n",
    )
    .unwrap();
    fs::write(business.join("Cargo.toml"), "[package]\nname='orders-business'\nversion='0.1.0'\nedition='2024'\n[package.metadata.ryframe]\nkind='business'\nmodule='orders'\n").unwrap();
    fs::write(business.join("src/lib.rs"), "").unwrap();
    fs::create_dir(root.path().join("config")).unwrap();
    let explicit = root.path().join("显式 配置");
    fs::create_dir(&explicit).unwrap();
    for directory in [root.path().join("config"), explicit.clone()] {
        let mut base: toml::Table =
            toml::from_str(include_str!("../../../config/app.toml")).unwrap();
        base.insert("snowflake_worker_id".into(), toml::Value::Integer(1));
        fs::write(directory.join("app.toml"), toml::to_string(&base).unwrap()).unwrap();
        fs::write(
            directory.join("app.dev.toml"),
            "[app]\nport = 'private-invalid-port'\n",
        )
        .unwrap();
    }
    for configured in [
        None,
        Some(explicit.clone()),
        Some(std::path::PathBuf::from("显式 配置")),
    ] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ryframe-generate"));
        command
            .args([
                "import",
                "--package",
                "orders-business",
                "--database",
                "control",
                "--connection",
                "primary",
                "--table",
                "table1",
            ])
            .current_dir(business.join("src"))
            .env("APP_ENV", "dev");
        if let Some(path) = &configured {
            command.arg("--config-dir").arg(path);
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(2));
        let error = String::from_utf8_lossy(&output.stderr);
        let expected = if configured.is_some() {
            explicit.clone()
        } else {
            root.path().join("config")
        };
        assert!(
            error.contains(&expected.join("app.dev.toml").display().to_string()),
            "{error}"
        );
        assert!(error.contains("app.port"), "{error}");
        assert!(!error.contains("private-invalid-port"), "{error}");
        assert!(!root.path().join("target").exists());
        assert!(!root.path().join(".local-tests").exists());
        assert!(!root.path().join("Cargo.lock").exists());
    }
}

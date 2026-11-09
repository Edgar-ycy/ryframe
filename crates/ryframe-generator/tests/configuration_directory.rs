use std::{fs, path::Path, process::Command};

use ryframe_generator::command::{GenerationBuilder, GenerationOperation};

fn builder(root: &Path) -> GenerationBuilder {
    GenerationBuilder::new(root)
        .operation(GenerationOperation::Resource)
        .package("orders-business")
}

#[test]
fn builder_resolves_default_public_local_and_explicit_directory_precedence() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(
        builder(root.path()).plan().unwrap().config_directory,
        root.path().join("config")
    );
    assert_eq!(
        builder(root.path()).plan().unwrap().target_dir,
        root.path().join("target")
    );
    fs::create_dir(root.path().join("config")).unwrap();
    fs::write(
        root.path().join("config/tools.toml"),
        "[generator]\nconfig_dir='公共 配置'\ntarget_dir='公共 缓存'\n",
    )
    .unwrap();
    assert_eq!(
        builder(root.path()).plan().unwrap().config_directory,
        root.path().join("公共 配置")
    );
    assert_eq!(
        builder(root.path()).plan().unwrap().target_dir,
        root.path().join("公共 缓存")
    );
    fs::create_dir(root.path().join(".local-tests")).unwrap();
    fs::write(
        root.path().join(".local-tests/tools.toml"),
        "[generator]\nconfig_dir='本机 配置'\ntarget_dir='本机 缓存'\n",
    )
    .unwrap();
    assert_eq!(
        builder(root.path()).plan().unwrap().config_directory,
        root.path().join("本机 配置")
    );
    assert_eq!(
        builder(root.path())
            .configuration_directory("显式 配置")
            .plan()
            .unwrap()
            .config_directory,
        root.path().join("显式 配置")
    );
    assert_eq!(
        builder(root.path()).plan().unwrap().target_dir,
        root.path().join("本机 缓存")
    );
    let cache = root.path().join("独立 缓存");
    let configured = format!(
        "[generator]\nconfig_dir='本机 配置'\ntarget_dir={}\n",
        serde_json::to_string(&cache).unwrap()
    );
    fs::write(root.path().join(".local-tests/tools.toml"), configured).unwrap();
    assert_eq!(builder(root.path()).plan().unwrap().target_dir, cache);
    let absolute = root.path().join("绝对 配置");
    assert_eq!(
        builder(root.path())
            .configuration_directory(&absolute)
            .plan()
            .unwrap()
            .config_directory,
        absolute
    );
    assert_eq!(
        builder(root.path())
            .configuration_directory("config")
            .plan()
            .unwrap()
            .config_directory,
        root.path().join("config")
    );
    assert!(!root.path().join("target").exists());
    assert_eq!(
        fs::read_dir(root.path().join(".local-tests"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn builder_and_tool_directories_reject_invalid_paths_before_execution() {
    let root = tempfile::tempdir().unwrap();
    for invalid in ["", " ", "bad\0path", "bad\rpath", "bad\npath"] {
        let error = builder(root.path())
            .configuration_directory(invalid)
            .plan()
            .unwrap_err();
        assert!(error.contains("config_dir"));
        assert!(!error.contains("bad"));
    }
    fs::create_dir(root.path().join("config")).unwrap();
    fs::write(
        root.path().join("config/tools.toml"),
        "[generator]\nconfig_dir=''\n",
    )
    .unwrap();
    assert!(
        builder(root.path())
            .plan()
            .unwrap_err()
            .contains("config_dir")
    );
    let mut invalid_targets = vec!["", " ", "bad\0path", "bad\tpath", "../outside"];
    if cfg!(windows) {
        invalid_targets.extend(["C:cache", r"\cache"]);
    }
    for invalid in invalid_targets {
        let mut settings = toml::Table::new();
        settings.insert("target_dir".into(), toml::Value::String(invalid.into()));
        let settings = toml::Table::from_iter([("generator".into(), toml::Value::Table(settings))]);
        fs::write(
            root.path().join("config/tools.toml"),
            toml::to_string(&settings).unwrap(),
        )
        .unwrap();
        assert!(
            builder(root.path())
                .plan()
                .unwrap_err()
                .contains("target_dir")
        );
    }
    let file = root.path().join("not-directory");
    fs::write(&file, b"existing file").unwrap();
    fs::write(
        root.path().join("config/tools.toml"),
        format!(
            "[generator]\ntarget_dir={}\n",
            serde_json::to_string(&file).unwrap()
        ),
    )
    .unwrap();
    assert!(builder(root.path()).plan().is_err());
    assert!(!root.path().join(".local-tests").exists());
}

#[test]
fn schema_source_and_generated_database_target_remain_independent() {
    let root = tempfile::tempdir().unwrap();
    for (source, database) in [
        ("sources.legacy", "tenant"),
        ("tenant_data.targets.legacy", "control"),
    ] {
        let plan = GenerationBuilder::new(root.path())
            .operation(GenerationOperation::Import)
            .package("orders-business")
            .source(source)
            .database(database)
            .include_tables(["table1", "table2"])
            .configuration_directory("独立 配置")
            .plan()
            .unwrap();
        assert_eq!(plan.source, source);
        assert_eq!(plan.database, database);
        assert_eq!(plan.tables, ["table1", "table2"]);
        assert_eq!(plan.config_directory, root.path().join("独立 配置"));
        assert!(plan.describe().contains(source));
    }
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn cli_from_business_src_preserves_tools_default_and_explicit_config_is_not_implicit() {
    let root = tempfile::tempdir().unwrap();
    let current = root.path().join("crates/orders-business/src");
    fs::create_dir_all(&current).unwrap();
    fs::write(
        root.path().join("Cargo.toml"),
        "[workspace]\nmembers=['crates/orders-business']\n",
    )
    .unwrap();
    fs::write(
        current.parent().unwrap().join("Cargo.toml"),
        "[package]\nname='orders-business'\nversion='0.1.0'\n",
    )
    .unwrap();
    fs::create_dir(root.path().join("config")).unwrap();
    fs::write(root.path().join("config/tools.toml"), "[generator]\npackage='orders-business'\nstorage='control'\ntables=['table1','table2']\nconfig_dir='公共 配置'\n").unwrap();
    let absolute = root.path().join("绝对 配置");
    let absolute_text = absolute.to_str().unwrap();
    let cases = [
        (vec!["run", "--plan"], root.path().join("公共 配置")),
        (
            vec!["--config-dir", "config", "run", "--plan"],
            root.path().join("config"),
        ),
        (
            vec!["run", "--plan", "--config-dir", absolute_text],
            absolute.clone(),
        ),
        (
            vec!["run", "--config-dir", "显式 配置", "--plan"],
            root.path().join("显式 配置"),
        ),
    ];
    for (arguments, expected) in cases {
        let output = Command::new(env!("CARGO_BIN_EXE_ryframe-generate"))
            .args(arguments)
            .current_dir(&current)
            .env("PATH", "")
            .env("APP_ENV", "must-not-read-for-plan")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let plan = String::from_utf8_lossy(&output.stdout);
        assert!(
            plan.contains(&format!("配置目录={}", expected.display())),
            "{plan}"
        );
        assert!(plan.contains("table1") && plan.contains("table2"));
        assert!(!expected.exists() || expected == root.path().join("config"));
        assert!(!root.path().join(".local-tests").exists());
        assert!(!root.path().join("Cargo.lock").exists());
        assert!(!root.path().join("target").exists());
    }
    fs::create_dir(root.path().join(".local-tests")).unwrap();
    fs::write(
        root.path().join(".local-tests/tools.toml"),
        "[generator]\nconfig_dir='本机 配置'\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_ryframe-generate"))
        .args(["run", "--plan"])
        .current_dir(&current)
        .env("PATH", "")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains(&format!(
        "配置目录={}",
        root.path().join("本机 配置").display()
    )));
    assert_eq!(
        fs::read_dir(root.path().join(".local-tests"))
            .unwrap()
            .count(),
        1
    );
}

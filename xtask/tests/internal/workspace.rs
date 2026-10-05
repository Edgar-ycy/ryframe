use super::workspace;

#[cfg(feature = "resource")]
use super::resource;

fn default_frontend_is_sibling_of_backend() {
    let root = workspace::root_dir();
    let expected = root.parent().unwrap().join("frontend");
    let expected = if expected.is_dir() {
        expected
    } else {
        root.parent().unwrap().join("ryframe-vue3")
    };
    assert_eq!(workspace::default_frontend_dir(), expected);
}

#[test]
fn isolated_directory_cleanup_rejects_its_root() {
    let root =
        std::env::temp_dir().join(format!("ryframe-workspace-cleanup-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let error = workspace::remove_isolated_directory(&root, &root)
        .unwrap_err()
        .to_string();
    assert!(error.contains("拒绝删除隔离边界外"));
    assert!(root.is_dir());
    std::fs::remove_dir(root).unwrap();
}

#[test]
fn daily_cargo_aliases_lock_dependencies_and_isolate_the_runner() {
    let config = std::fs::read_to_string(workspace::root_dir().join(".cargo/config.toml")).unwrap();
    assert!(
        config.lines().any(|line| {
            line.starts_with("xtask = \"run --locked --target-dir target/xtask-run ")
        })
    );
    for removed in ["dev", "verify", "resource", "api-sync"] {
        assert!(
            !config
                .lines()
                .any(|line| line.starts_with(&format!("{removed} =")))
        );
    }
    for (alias, command) in [
        ("serve", "run -p ryframe --bin ryframe"),
        (
            "worker",
            "run -p ryframe --bin ryframe-worker --no-default-features --features bin-worker",
        ),
        (
            "migrate",
            "run -p ryframe --bin ryframe-migrate --no-default-features --features bin-migrate",
        ),
        (
            "reset",
            "run -p ryframe --bin ryframe-reset --no-default-features --features bin-reset",
        ),
        (
            "generate",
            "run -p ryframe-generator --features schema-import --bin ryframe-generate --",
        ),
    ] {
        assert!(
            config
                .lines()
                .any(|line| line == format!("{alias} = \"{command}\""))
        );
    }
}

#[cfg(feature = "resource")]
#[test]
fn loads_and_explains_catalog_resources() {
    let resources = resource::load_catalog(&workspace::root_dir()).unwrap();
    let catalog = ryframe_generator::render_resources(&resources).unwrap();
    assert!(catalog.explanation("post").is_some());
}

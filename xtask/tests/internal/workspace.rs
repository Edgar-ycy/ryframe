use super::workspace;

#[cfg(feature = "resource")]
use super::resource;

fn default_frontend_is_sibling_of_backend() {
    let root = workspace::root_dir();
    assert_eq!(
        workspace::default_frontend_dir(),
        root.parent().unwrap().join("ryframe-vue3")
    );
}

#[test]
fn daily_cargo_aliases_lock_dependencies_and_isolate_the_runner() {
    let config = std::fs::read_to_string(workspace::root_dir().join(".cargo/config.toml")).unwrap();
    for alias in ["xtask", "dev", "verify", "api-sync", "migrate"] {
        let prefix = format!("{alias} = \"run --locked --target-dir target/xtask-run ");
        assert!(config.lines().any(|line| line.starts_with(&prefix)));
    }
    assert!(config.lines().any(|line| {
        line.starts_with("resource = \"run --locked --target-dir target/xtask-resource ")
    }));
}

#[cfg(feature = "resource")]
#[test]
fn loads_and_explains_catalog_resources() {
    let resources = resource::load_catalog(&workspace::root_dir()).unwrap();
    let catalog = ryframe_generator::render_resources(&resources).unwrap();
    assert!(catalog.explanation("post").is_some());
}

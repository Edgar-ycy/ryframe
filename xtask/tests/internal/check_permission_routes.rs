use std::{
    fs,
    path::{Path, PathBuf},
};

use super::{
    check::{TaskExecutor, permission_routes},
    process::failure_exit_code,
};

#[test]
fn permission_scan_reports_every_forbidden_boundary_with_relative_paths() {
    let fixture = Fixture::new();
    fixture.write(
        "crates/ryframe-api/src/router.rs",
        concat!(
            "generator_handler();\n",
            "let path = \"/api/v1/tools/gen\";\n",
            "let segment = \"/gen\";\n",
            "let permission = \"tools:gen:write\";\n",
            "let menu = \"tools.gen\";\n",
        ),
    );
    let violations = permission_routes::violations(&fixture.root).unwrap();
    assert_eq!(violations.len(), 5, "{violations:#?}");
    assert!(
        violations
            .iter()
            .all(|item| item.starts_with("crates/ryframe-api/src/router.rs :: "))
    );
    assert!(
        violations
            .iter()
            .all(|item| !item.contains('\\') && !item.contains(&fixture.root.display().to_string()))
    );
    let before = fixture.snapshot();
    let error = permission_routes::run(&fixture.root).unwrap_err();
    assert!(error.to_string().contains("发现 5 处违规"));
    assert_eq!(failure_exit_code(error.as_ref()), Some(1));
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn permission_scan_excludes_the_offline_generator_crate_and_accepts_product_code() {
    let fixture = Fixture::new();
    fixture.write(
        "crates/ryframe-generator/src/lib.rs",
        "generator_handler(); let path = \"/gen\";\n",
    );
    fixture.write(
        "crates/ryframe-api/src/router.rs",
        "let route = \"/api/v1/posts\";\n",
    );
    assert!(
        permission_routes::violations(&fixture.root)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        TaskExecutor::PermissionRoutes.definition().id,
        "policy.permission-routes"
    );
    assert!(
        TaskExecutor::PermissionRoutes
            .definition()
            .allowed_writes
            .contains(&"Cargo target")
    );
}

#[test]
fn current_product_sources_satisfy_the_permission_boundary_without_writes() {
    let root = super::workspace::root_dir();
    let before = snapshot_product_sources(&root);
    assert!(permission_routes::violations(&root).unwrap().is_empty());
    assert_eq!(snapshot_product_sources(&root), before);
}

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("target/permission-routes-tests");
        fs::create_dir_all(&parent).unwrap();
        let temporary = tempfile::tempdir_in(parent).unwrap();
        let root = temporary.path().to_path_buf();
        fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
        fs::create_dir_all(root.join("crates/ryframe-api/src")).unwrap();
        Self {
            _temporary: temporary,
            root,
        }
    }

    fn write(&self, relative: &str, source: &str) {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, source).unwrap();
    }

    fn snapshot(&self) -> Vec<(PathBuf, Vec<u8>)> {
        snapshot_paths(&self.root, &["crates", "Cargo.toml"])
    }
}

fn snapshot_paths(root: &Path, paths: &[&str]) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    for relative in paths {
        let path = root.join(relative);
        if path.is_file() {
            files.push((PathBuf::from(relative), fs::read(path).unwrap()));
        } else {
            collect(root, &path, &mut files);
        }
    }
    files.sort();
    files
}

fn snapshot_product_sources(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    for entry in fs::read_dir(root.join("crates")).unwrap() {
        let source = entry.unwrap().path().join("src");
        if source.is_dir() {
            collect_rust(root, &source, &mut files);
        }
    }
    files.sort();
    files
}

fn collect_rust(root: &Path, path: &Path, files: &mut Vec<(PathBuf, Vec<u8>)>) {
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_rust(root, &path, files);
        } else if path.extension().and_then(|value| value.to_str()) == Some("rs") {
            files.push((
                path.strip_prefix(root).unwrap().to_path_buf(),
                fs::read(path).unwrap(),
            ));
        }
    }
}

fn collect(root: &Path, path: &Path, files: &mut Vec<(PathBuf, Vec<u8>)>) {
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect(root, &path, files);
        } else {
            files.push((
                path.strip_prefix(root).unwrap().to_path_buf(),
                fs::read(path).unwrap(),
            ));
        }
    }
}

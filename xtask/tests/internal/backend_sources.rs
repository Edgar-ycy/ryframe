use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use super::check::check_backend_sources;

struct Fixture(tempfile::TempDir);

impl Fixture {
    fn new() -> Self {
        let local = backend_root().join(".local-tests/backend-sources");
        fs::create_dir_all(&local).unwrap();
        let fixture = Self(tempfile::tempdir_in(local).unwrap());
        fixture.git(&["init", "--quiet"]);
        fixture
    }

    fn root(&self) -> &Path {
        self.0.path()
    }

    fn write(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.root().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }

    fn git(&self, arguments: &[&str]) {
        let output = Command::new("git")
            .args(arguments)
            .current_dir(self.root())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn current_backend_has_no_self_maintained_javascript_or_typescript() {
    check_backend_sources(backend_root()).unwrap();
}

#[test]
fn rejects_all_script_extensions_in_tracked_and_untracked_paths() {
    let fixture = Fixture::new();
    let paths = [
        "src/a.js",
        "tools/b.mjs",
        "tests/c.cjs",
        "templates/d.jsx",
        "nested/e.ts",
        "nested/f.mts",
        "nested/g.cts",
        "nested/h.tsx",
        "中文目录/i.TS",
    ];
    for path in paths {
        fixture.write(path, "");
    }
    fixture.git(&["add", "src/a.js"]);
    let message = check_backend_sources(fixture.root())
        .unwrap_err()
        .to_string();
    for path in paths {
        assert_eq!(
            message.matches(path).count(),
            1,
            "违规路径必须恰好出现一次：{path}；{message}"
        );
    }
    let mut ordered = paths;
    ordered.sort_unstable();
    assert!(
        ordered
            .windows(2)
            .all(|pair| message.find(pair[0]) < message.find(pair[1]))
    );
    assert_eq!(
        message,
        check_backend_sources(fixture.root())
            .unwrap_err()
            .to_string()
    );
    assert!(message.contains("Rust"));
}

#[test]
fn vendor_ignored_and_deleted_files_are_excluded_without_hiding_other_sources() {
    let fixture = Fixture::new();
    fixture.write(".gitignore", "ignored/\n");
    fixture.write("vendor/upstream/source.js", "");
    fixture.write("ignored/cache.ts", "");
    let deleted = fixture.write("tools/deleted.mjs", "");
    fixture.write("src/lib.rs", "");
    fixture.git(&["add", "."]);
    fs::remove_file(deleted).unwrap();
    check_backend_sources(fixture.root()).unwrap();
    fixture.write("vendor-copy/not-upstream.ts", "");
    let message = check_backend_sources(fixture.root())
        .unwrap_err()
        .to_string();
    assert!(message.contains("vendor-copy/not-upstream.ts"));
    assert!(!message.contains("vendor/upstream"));
}

fn backend_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

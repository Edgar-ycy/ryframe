use std::{fs, path::Path, process::Command};

use serde_json::{Value, json};
use tempfile::TempDir;

use crate::recovery::{clone::native::source, source::evidence::staging};

struct Fixture {
    _temporary: TempDir,
    args: Value,
}

fn git(root: &Path, args: &[&str]) -> String {
    let result = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().into()
}

impl Fixture {
    fn new() -> Self {
        let base = crate::workspace::root_dir().join(".local-tests/source-evidence-unit");
        fs::create_dir_all(&base).unwrap();
        let temporary = tempfile::tempdir_in(base).unwrap();
        let root = temporary.path();
        fs::create_dir_all(root.join("openapi")).unwrap();
        fs::write(root.join("openapi/openapi.json"), b"{\"paths\":{}}\n").unwrap();
        fs::write(root.join(".gitignore"), ".local-tests/\n").unwrap();
        git(root, &["init", "--quiet"]);
        git(root, &["add", "."]);
        git(
            root,
            &[
                "-c",
                "user.name=Source test",
                "-c",
                "user.email=source@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "fixture",
            ],
        );
        let inventory = source::capture(root).unwrap();
        let coordinator = source::execution_source(&inventory).unwrap();
        let tool_path = std::env::current_exe().unwrap();
        let hash = crate::recovery::clone::native::digest::file(&tool_path)
            .unwrap()
            .sha256;
        let directory = root.join(".local-tests/run/verification");
        fs::create_dir_all(&directory).unwrap();
        let args = json!({"coordinator":root,"execution":root,"directory":directory,
            "coordinator_source":coordinator,"execution_sha":inventory.guard.head,
            "tool":{"path":tool_path,"sha256":hash}});
        Self {
            _temporary: temporary,
            args,
        }
    }

    fn create(&self) -> Value {
        let mut args = self.args.clone();
        args["descriptor"] = staging::create(&self.args).unwrap();
        args
    }

    fn root(&self) -> std::path::PathBuf {
        Path::new(self.args["directory"].as_str().unwrap()).join("source-tools")
    }
}

#[test]
fn immutable_git_blob_and_one_native_tool_define_exact_create_only_staging() {
    let fixture = Fixture::new();
    // 可变checkout故意不同，仍然只能复制登记commit中的契约。
    let contract =
        Path::new(fixture.args["execution"].as_str().unwrap()).join("openapi/openapi.json");
    fs::write(contract, b"mutable checkout must not be used").unwrap();
    let args = fixture.create();
    let value = staging::verify(&args).unwrap();
    assert_eq!(
        value["manifest"]["runtime_modules"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        value["manifest"]["runtime_modules"][0]["path"],
        args["tool"]["path"]
    );
    assert_eq!(
        fs::read(fixture.root().join("openapi/openapi.json")).unwrap(),
        b"{\"paths\":{}}\n"
    );
    assert!(staging::create(&fixture.args).is_err());
}

#[test]
fn staging_rejects_blob_manifest_tool_revision_and_file_set_drift() {
    let fixture = Fixture::new();
    let args = fixture.create();
    let contract = fixture.root().join("openapi/openapi.json");
    let raw = fs::read(&contract).unwrap();
    fs::write(&contract, b"drift").unwrap();
    assert!(
        staging::verify(&args)
            .unwrap_err()
            .to_string()
            .contains("Git blob")
    );
    fs::write(&contract, raw).unwrap();
    for name in ["unknown.txt", "openapi/unknown.txt"] {
        let path = fixture.root().join(name);
        fs::write(&path, "unknown").unwrap();
        assert!(staging::verify(&args).is_err());
        fs::remove_file(path).unwrap();
    }
    let unknown = fixture.root().join("empty-unknown");
    fs::create_dir(&unknown).unwrap();
    assert!(staging::verify(&args).is_err());
    fs::remove_dir(unknown).unwrap();
    for (path, value) in [
        ("/tool/sha256", json!("f".repeat(64))),
        ("/execution_sha", json!("f".repeat(40))),
        ("/coordinator_source/snapshot/clean", json!(false)),
        ("/descriptor/path", json!("outside")),
    ] {
        let mut changed = args.clone();
        *changed.pointer_mut(path).unwrap() = value;
        assert!(staging::verify(&changed).is_err(), "{path}");
    }
    assert!(staging::verify(&args).is_ok());
}

#[test]
fn interrupted_or_racing_creation_is_preserved_and_never_replayed() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.root()).unwrap();
    fs::write(fixture.root().join("partial.txt"), "preserve me").unwrap();
    assert!(staging::create(&fixture.args).is_err());
    assert_eq!(
        fs::read(fixture.root().join("partial.txt")).unwrap(),
        b"preserve me"
    );
    let fixture = Fixture::new();
    let first = fixture.args.clone();
    let second = first.clone();
    let one = std::thread::spawn(move || staging::create(&first).is_ok());
    let two = std::thread::spawn(move || staging::create(&second).is_ok());
    assert_ne!(one.join().unwrap(), two.join().unwrap());
}

#[test]
fn invalid_revision_and_outside_directory_fail_before_any_write() {
    let fixture = Fixture::new();
    for value in [
        json!("HEAD"),
        json!("a".repeat(39)),
        json!("-option"),
        json!(true),
    ] {
        let mut args = fixture.args.clone();
        args["execution_sha"] = value;
        assert!(staging::create(&args).is_err());
        assert!(!fixture.root().exists());
    }
    let mut args = fixture.args.clone();
    args["directory"] = args["coordinator"].clone();
    assert!(staging::create(&args).is_err());
    assert!(!fixture.root().exists());
}

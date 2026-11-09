use super::{builds, common, document};
use crate::recovery::clone::native::{digest, source};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

struct Fixture {
    backend: PathBuf,
    execution: PathBuf,
    runtime: Value,
}
impl Fixture {
    fn new() -> Self {
        let backend = crate::workspace::root_dir();
        let execution = backend
            .join(".local-tests")
            .join(format!("generation-build-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(execution.join(".local-tests")).unwrap();
        for (name, content) in [
            (".gitignore", ".local-tests/\ntarget/\n"),
            ("Cargo.toml", "[workspace]\n"),
            ("Cargo.lock", "version=4\n"),
            ("product/input.txt", "product\n"),
            ("crates/ryframe/Cargo.toml", "[package]\nname='ryframe'\n"),
            ("crates/ryframe/src/main.rs", "fn main() {}\n"),
            ("crates/ryframe-api/src/lib.rs", "pub fn api() {}\n"),
        ] {
            let path = execution.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }
        for args in [
            vec!["init", "--quiet"],
            vec!["config", "user.name", "seed-test"],
            vec!["config", "user.email", "seed@example.invalid"],
            vec!["config", "core.autocrlf", "false"],
            vec!["add", "."],
            vec!["commit", "--quiet", "-m", "fixture"],
        ] {
            let output = Command::new("git")
                .args(args)
                .current_dir(&execution)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let inventory = source::capture(&execution).unwrap();
        let mut artifacts = serde_json::Map::new();
        let mut commands = serde_json::Map::new();
        for (role, feature, name) in [
            ("api", "bin-api", "ryframe"),
            ("worker", "bin-worker", "ryframe-worker"),
        ] {
            let executable = execution.join(".local-tests").join(format!("{name}.exe"));
            fs::write(&executable, format!("fixed {role}")).unwrap();
            let file = digest::file(&executable).unwrap();
            let command = build_command(feature, name);
            commands.insert(role.into(), json!(command));
            artifacts.insert(role.into(), json!({"executable":executable,"command":command,"bytes":file.bytes,"sha256":file.sha256}));
        }
        let runtime = json!({"format_version":2,"kind":"restore-backend-build",
            "sources":source::backend_domains(&inventory).unwrap(),"build":{"commands":commands,
            "profile":"dev","target":"fixture-windows","jobs":"4","toolchain":{"cargo":"fixed","rustc":"fixed"},
            "environment":{"variables":[],"sha256":"a".repeat(64)}},"artifacts":artifacts});
        Self {
            backend,
            execution,
            runtime,
        }
    }

    fn runtime_receipt(&self, name: &str, hash: &str) -> Value {
        let mut value = self.runtime.clone();
        value["build"]["environment"]["sha256"] = hash.into();
        let path = self
            .execution
            .join(".local-tests")
            .join(format!("{name}.json"));
        document::write_new(&path, &value).unwrap();
        common::binding(&path).unwrap()
    }

    fn maintenance(&self, name: &str, log_text: &str) -> Value {
        let directory = self.execution.join(".local-tests").join(name);
        let target = self.execution.join("target").join("ci").join("backend");
        fs::create_dir_all(&directory).unwrap();
        fs::create_dir_all(&target).unwrap();
        let mut artifacts = serde_json::Map::new();
        for (role, feature, binary) in [
            ("reset", "bin-reset", "ryframe-reset"),
            ("migrate", "bin-migrate", "ryframe-migrate"),
            ("tenant-data", "bin-tenant-data", "ryframe-tenant-data"),
        ] {
            let original = target.join(format!("{binary}.exe"));
            fs::write(&original, format!("fixed {binary}")).unwrap();
            let saved = directory.join(format!("{binary}.exe"));
            fs::copy(&original, &saved).unwrap();
            let output = directory.join(format!("{role}.cargo.jsonl"));
            let log = directory.join(format!("{role}.cargo.log"));
            let event = json!({"reason":"compiler-artifact","target":{"name":binary,"kind":["bin"]},
                "manifest_path":self.execution.join("crates/ryframe/Cargo.toml"),"executable":original});
            fs::write(&output, format!("{event}\n")).unwrap();
            fs::write(&log, log_text).unwrap();
            let mut command = build_command(feature, binary);
            command.pop();
            command.truncate(command.len() - 2);
            command.extend([
                "--jobs".into(),
                "4".into(),
                "--target-dir".into(),
                target.to_str().unwrap().into(),
                "--message-format=json".into(),
            ]);
            let file = digest::file(&saved).unwrap();
            artifacts.insert(role.into(),json!({"executable":saved,"cargo_executable":original,"command":command,
                "bytes":file.bytes,"sha256":file.sha256,"cargo_output":log_binding(&output),"cargo_log":log_binding(&log)}));
        }
        let value = json!({"format_version":1,"kind":"devex-clone-tool-build","backend_root":self.execution,
            "source":self.runtime["sources"]["full"]["source"],"source_inventory":self.runtime["sources"]["full"],
            "toolchain":{"cargo":"fixed","rustc":"fixed"},"target_directory":target,"artifacts":artifacts,
            "restore_qualified":false,"resources_modified":false});
        let path = directory.join("build.json");
        document::write_new(&path, &value).unwrap();
        common::binding(&path).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(
            self.execution
                .starts_with(self.backend.join(".local-tests"))
        );
        let cleanup = crate::ci::resource_gate::replay::remove_partial_worktree_directory(
            &self.execution,
            &self.backend.join(".local-tests"),
        );
        if !std::thread::panicking() {
            cleanup.unwrap();
        }
    }
}
fn build_command(feature: &str, name: &str) -> Vec<String> {
    [
        "cargo",
        "build",
        "--locked",
        "-p",
        "ryframe",
        "--no-default-features",
        "--features",
        feature,
        "--bin",
        name,
        "--jobs",
        "4",
        "--message-format=json",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}
fn log_binding(path: &Path) -> Value {
    let file = digest::file(path).unwrap();
    json!({"file":path.file_name().unwrap().to_str().unwrap(),"bytes":file.bytes,"sha256":file.sha256})
}

#[test]
fn complete_bound_build_pair_verifies_actual_artifacts_logs_and_original_receipts() {
    let fixture = Fixture::new();
    let old = json!({"backend_build":fixture.runtime_receipt("old-runtime", &"a".repeat(64)),
        "maintenance_build":fixture.maintenance("old-maintenance", "old diagnostic")});
    let current = json!({"backend_build":fixture.runtime_receipt("current-runtime", &"b".repeat(64)),
        "maintenance_build":fixture.maintenance("current-maintenance", "new diagnostic"),
        "execution_backend":fixture.execution,"expected_backend_sha":fixture.runtime["sources"]["full"]["source"]["snapshot"]["head"]});
    builds::verify(&fixture.backend, &old, &current).unwrap();
    for name in [
        ".local-tests/ryframe.exe",
        ".local-tests/current-maintenance/ryframe-reset.exe",
        ".local-tests/current-maintenance/reset.cargo.log",
        ".local-tests/current-runtime.json",
    ] {
        let path = fixture.execution.join(name);
        let before = fs::read(&path).unwrap();
        fs::write(&path, b"tampered").unwrap();
        assert!(
            builds::verify(&fixture.backend, &old, &current).is_err(),
            "{name}"
        );
        fs::write(&path, before).unwrap();
    }
    builds::verify(&fixture.backend, &old, &current).unwrap();
    assert!(!fixture.execution.join("intent.json").exists());
}

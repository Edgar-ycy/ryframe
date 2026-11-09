use super::*;
use serde_json::Value;
use std::fs;

#[test]
fn file_target_controls_real_cargo_metadata_and_recorded_environment_without_creating_target() {
    let base = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&base).unwrap();
    let directory = tempfile::tempdir_in(base).unwrap();
    let root = directory.path();
    fs::create_dir(root.join("src")).unwrap();
    fs::create_dir(root.join(".local-tests")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='cargo-target-probe'\nversion='0.0.0'\nedition='2024'\n[workspace]\n",
    )
    .unwrap();
    fs::write(root.join("src/lib.rs"), "").unwrap();
    let lock = "version = 4\n\n[[package]]\nname = \"cargo-target-probe\"\nversion = \"0.0.0\"\n";
    fs::write(root.join("Cargo.lock"), lock).unwrap();
    let inherited = BTreeMap::from([("CARGO_TARGET_DIR".into(), "inherited-target".into())]);
    let mut effective = inherited.clone();
    assert_eq!(apply(root, &mut effective).unwrap(), None);
    assert_eq!(effective, inherited);

    let config = root.join(".local-tests/tools.toml");
    fs::write(&config, "[generator]\ntarget_dir='共享 target'\n").unwrap();
    let expected = root.join("共享 target");
    apply(root, &mut effective).unwrap();
    assert_eq!(Path::new(&effective["CARGO_TARGET_DIR"]), expected);
    let mut command = Command::new("cargo");
    command
        .current_dir(root)
        .args([
            "metadata",
            "--locked",
            "--offline",
            "--no-deps",
            "--format-version",
            "1",
        ])
        .env("CARGO_TARGET_DIR", "wrong-target");
    configure_command(root, &mut command).unwrap();
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        Path::new(metadata["target_directory"].as_str().unwrap()),
        expected
    );
    assert!(!expected.exists());
    assert!(!root.join("wrong-target").exists());
    assert_eq!(fs::read_to_string(root.join("Cargo.lock")).unwrap(), lock);

    let rustc = "rustc fixture\nhost: x86_64-pc-windows-msvc\n";
    let first =
        super::super::build_context::from_inputs("cargo fixture", rustc, &effective, 4).unwrap();
    assert_eq!(
        first["environment"]["variables"],
        serde_json::json!(["CARGO_TARGET_DIR"])
    );
    assert!(!first.to_string().contains("共享 target"));
    let mut differently_inherited =
        BTreeMap::from([("CARGO_TARGET_DIR".into(), "another-parent-target".into())]);
    apply(root, &mut differently_inherited).unwrap();
    let same =
        super::super::build_context::from_inputs("cargo fixture", rustc, &differently_inherited, 4)
            .unwrap();
    assert_eq!(first, same);
    fs::write(config, "[generator]\ntarget_dir='other-target'\n").unwrap();
    apply(root, &mut effective).unwrap();
    let changed =
        super::super::build_context::from_inputs("cargo fixture", rustc, &effective, 4).unwrap();
    assert_ne!(first["environment"], changed["environment"]);
}

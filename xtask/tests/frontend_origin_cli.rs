//! 原生 origin 工具的真实二进制协议与来源绑定测试。

use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const SENTINEL: &str = "FRONTEND_ORIGIN_SECRET_SENTINEL_7c918d";
const INPUT_LIMIT: usize = 64 * 1024 * 1024;

struct Fixture(tempfile::TempDir);

impl Fixture {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".local-tests/frontend-origin-cli");
        fs::create_dir_all(&root).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("case-")
            .tempdir_in(root)
            .unwrap();
        fs::create_dir(directory.path().join("config")).unwrap();
        fs::write(directory.path().join("config/app.toml"), SENTINEL).unwrap();
        fs::write(directory.path().join("preserved.bin"), [0_u8, 255, 42]).unwrap();
        Self(directory)
    }

    fn invoke(&self, arguments: &[&str], input: &[u8], expected_code: i32) -> Value {
        let before = inventory(self.0.path());
        let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
        command
            .args(arguments)
            .current_dir(self.0.path())
            .env("APP_ENV", SENTINEL)
            .env("APP_CONFIG_DIR", SENTINEL)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(input).unwrap();
        drop(stdin);
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(expected_code));
        assert!(output.stderr.is_empty());
        assert!(!String::from_utf8_lossy(&output.stdout).contains(SENTINEL));
        assert_eq!(inventory(self.0.path()), before);
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["format_version"], 1);
        value
    }

    fn check(&self, input: &[u8], expected_code: i32) -> Value {
        self.invoke(&["__frontend-origin", "check"], input, expected_code)
    }
}

fn inventory(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    let mut entries = BTreeMap::new();
    let mut pending = vec![root.to_owned()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap().to_owned();
            let metadata = fs::symlink_metadata(&path).unwrap();
            assert!(!metadata.file_type().is_symlink());
            if metadata.is_dir() {
                entries.insert(relative, None);
                pending.push(path);
            } else {
                entries.insert(relative, Some(fs::read(path).unwrap()));
            }
        }
    }
    entries
}

fn request(value: &str, allow_empty: bool) -> Vec<u8> {
    serde_json::to_vec(&json!({"format_version": 1, "value": value, "allow_empty": allow_empty}))
        .unwrap()
}

fn assert_failure(value: &Value) {
    assert_eq!(value.as_object().unwrap().len(), 3);
    assert_eq!(value["valid"], false);
    let error = value["error"].as_str().unwrap();
    assert!(error.contains('：'));
    assert!(
        error
            .chars()
            .any(|character| ('\u{4e00}'..='\u{9fff}').contains(&character))
    );
}

#[test]
fn real_binary_returns_normalized_origins_and_empty_policy() {
    let fixture = Fixture::new();
    let cases = [
        ("HTTPS://EXAMPLE.INVALID:443/", "https://example.invalid"),
        ("http://example.invalid/?", "http://example.invalid"),
        ("http://example.invalid/#", "http://example.invalid"),
        ("http://example.invalid/?#", "http://example.invalid"),
        ("http://:@example.invalid/", "http://example.invalid"),
        (r"http:\\example.invalid\", "http://example.invalid"),
        ("http://127.1", "http://127.0.0.1"),
        ("http://0x7f000001", "http://127.0.0.1"),
        ("http://[::1]:8080/", "http://[::1]:8080"),
        ("https://例子.测试", "https://xn--fsqu00a.xn--0zwm56d"),
        (
            "\u{feff}\u{a0}https://example.invalid/\u{3000}",
            "https://example.invalid",
        ),
    ];
    for (input, expected) in cases {
        let value = fixture.check(&request(input, false), 0);
        assert_eq!(
            value,
            json!({"format_version": 1, "valid": true, "origin": expected})
        );
    }
    let value = fixture.check(&request(" \u{feff}\t", true), 0);
    assert_eq!(
        value,
        json!({"format_version": 1, "valid": true, "origin": ""})
    );
    assert_failure(&fixture.check(&request(" \u{feff}\t", false), 1));
}

#[test]
fn real_binary_rejects_bad_urls_without_disclosing_input_or_userinfo() {
    let fixture = Fixture::new();
    let cases = [
        format!("http://{SENTINEL}:password@example.invalid/"),
        format!("http://:{SENTINEL}@example.invalid/"),
        format!("http://example.invalid/{SENTINEL}"),
        format!("http://example.invalid/?{SENTINEL}"),
        format!("http://example.invalid/#{SENTINEL}"),
        format!("invalid-{SENTINEL}"),
        "ftp://example.invalid/".into(),
        "http://999.999.999.999".into(),
        "http://example.invalid:65536/".into(),
        "http://example.invalid:invalid/".into(),
        "\u{85}http://example.invalid/".into(),
    ];
    for input in cases {
        assert_failure(&fixture.check(&request(&input, false), 1));
    }
}

#[test]
fn real_binary_enforces_exact_json_fields_types_and_version() {
    let fixture = Fixture::new();
    let cases = [
        "".to_owned(),
        "null".to_owned(),
        "[]".to_owned(),
        r#"[1,"",true]"#.to_owned(),
        "{}".to_owned(),
        format!(r#"{{"format_version":1,"value":"{SENTINEL}","allow_empty":false,"extra":true}}"#),
        r#"{"format_version":1,"format_version":1,"value":"","allow_empty":true}"#.into(),
        r#"{"format_version":1,"value":"","value":"","allow_empty":true}"#.into(),
        r#"{"format_version":1,"value":"","allow_empty":true,"allow_empty":true}"#.into(),
        r#"{"format_version":2,"value":"","allow_empty":true}"#.into(),
        r#"{"format_version":1.0,"value":"","allow_empty":true}"#.into(),
        r#"{"format_version":true,"value":"","allow_empty":true}"#.into(),
        r#"{"format_version":"1","value":"","allow_empty":true}"#.into(),
        r#"{"format_version":1,"value":null,"allow_empty":true}"#.into(),
        r#"{"format_version":1,"value":123,"allow_empty":true}"#.into(),
        r#"{"format_version":1,"value":"","allow_empty":"true"}"#.into(),
        r#"{"format_version":1,"value":"","allow_empty":true}{}"#.into(),
        format!("{{{SENTINEL}"),
    ];
    for input in cases {
        assert_failure(&fixture.check(input.as_bytes(), 2));
    }
    let mut invalid_utf8 = request(SENTINEL, false);
    let position = invalid_utf8.iter().position(|byte| *byte == b'F').unwrap();
    invalid_utf8[position] = 0xff;
    assert_failure(&fixture.check(&invalid_utf8, 2));
}

#[test]
fn real_binary_rejects_missing_unknown_and_extra_arguments() {
    let fixture = Fixture::new();
    for arguments in [
        vec!["__frontend-origin"],
        vec!["__frontend-origin", SENTINEL],
        vec!["__frontend-origin", "check", SENTINEL],
        vec!["__frontend-origin", "describe", SENTINEL],
        vec!["__frontend-origin", "--help"],
    ] {
        assert_failure(&fixture.invoke(&arguments, &[], 2));
    }
}

#[test]
fn real_binary_accepts_exact_input_limit_and_rejects_one_more_byte() {
    let fixture = Fixture::new();
    let mut input = request("http://example.invalid/?", false);
    input.resize(INPUT_LIMIT, b' ');
    assert_eq!(fixture.check(&input, 0)["origin"], "http://example.invalid");
    input.push(b' ');
    let value = fixture.check(&input, 2);
    assert_failure(&value);
    assert!(value["error"].as_str().unwrap().contains("64 MiB"));
}

#[test]
fn describe_binds_real_executable_and_all_compiled_source_bytes() {
    let fixture = Fixture::new();
    let value = fixture.invoke(&["__frontend-origin", "describe"], &[], 0);
    assert_eq!(value.as_object().unwrap().len(), 4);
    assert_eq!(value["kind"], "frontend-origin-tool");
    let executable = &value["executable"];
    assert_eq!(executable.as_object().unwrap().len(), 2);
    let actual_path = Path::new(executable["path"].as_str().unwrap());
    assert!(actual_path.is_absolute());
    assert!(actual_path.is_file());
    let canonical_path = fs::canonicalize(env!("CARGO_BIN_EXE_xtask")).unwrap();
    assert_eq!(fs::canonicalize(actual_path).unwrap(), canonical_path);
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};

        if matches!(
            canonical_path.components().next(),
            Some(Component::Prefix(prefix))
                if matches!(prefix.kind(), Prefix::VerbatimDisk(_) | Prefix::Disk(_))
        ) {
            assert!(matches!(
                actual_path.components().next(),
                Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_))
            ));
        }
    }
    let canonical_hash = hex::encode(Sha256::digest(fs::read(&canonical_path).unwrap()));
    assert_eq!(executable["sha256"], canonical_hash);
    assert_eq!(
        executable["sha256"],
        hex::encode(Sha256::digest(fs::read(actual_path).unwrap()))
    );
    let sources = value["sources"].as_array().unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    assert!(sources.len() > 100);
    for required in [
        "xtask/src/recovery/full_stack/worker/mod.rs",
        "Cargo.toml",
        "xtask/build.rs",
        "xtask/source-inventory.json",
        "crates/ryframe-config/src/lib.rs",
    ] {
        assert!(
            sources.iter().any(|source| source["path"] == required),
            "缺少 {required}"
        );
    }
    let mut previous = "";
    for source in sources {
        assert_eq!(source.as_object().unwrap().len(), 2);
        let path = source["path"].as_str().unwrap();
        assert!(path > previous);
        previous = path;
        assert_eq!(
            source["sha256"],
            hex::encode(Sha256::digest(fs::read(root.join(path)).unwrap()))
        );
    }
}

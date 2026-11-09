use std::{fs, path::PathBuf};

use serde_json::{Value, json};
use tempfile::TempDir;

use super::{compare, executable};

struct Fixture {
    _directory: TempDir,
    first: PathBuf,
    second: PathBuf,
    observed: Value,
    target: Value,
}

impl Fixture {
    fn new() -> Self {
        let parent = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&parent).unwrap();
        let holder = tempfile::Builder::new()
            .prefix("seed-storage-compare-")
            .tempdir_in(parent)
            .unwrap();
        let first = holder.path().join("rustfs.exe");
        let second = holder.path().join("other.exe");
        fs::write(&first, "ordinary fixture file; no process").unwrap();
        fs::write(&second, "different fixture file; no process").unwrap();
        let observed = json!({"api_url":"http://127.0.0.1:19200",
            "storage":{"sha256":"a".repeat(64),"identity":{"executable":first}}});
        let target = json!({"target":{"s3":{"endpoint":"http://127.0.0.1:19200"}},
            "storage":{"rustfs":{"sha256":"a".repeat(64),"identity":{"executable":first}}}});
        Self {
            _directory: holder,
            first,
            second,
            observed,
            target,
        }
    }

    fn check(&self) -> crate::Result<()> {
        compare(
            &self.observed,
            &self.target,
            &executable(&self.observed["storage"]["identity"]["executable"])?,
            &executable(&self.target["storage"]["rustfs"]["identity"]["executable"])?,
        )
    }
}

#[test]
fn exact_observation_accepts_canonical_equivalent_spelling_without_a_live_process() {
    let mut fixture = Fixture::new();
    // 冗余分隔符是同一合法路径拼写，未引入链接、父目录跳转或伪造PID。
    let parent = fixture.first.parent().unwrap().to_str().unwrap();
    let spelling = format!(
        "{parent}{}rustfs.exe",
        std::path::MAIN_SEPARATOR_STR.repeat(2)
    );
    fixture.target["storage"]["rustfs"]["identity"]["executable"] = json!(spelling);
    assert_ne!(
        fixture.observed["storage"]["identity"]["executable"],
        fixture.target["storage"]["rustfs"]["identity"]["executable"]
    );
    assert_eq!(
        executable(&json!(fixture.first)).unwrap(),
        executable(&json!(spelling)).unwrap()
    );
    assert!(fixture.check().is_ok());
}

#[test]
fn endpoint_hash_and_actual_executable_must_each_match_the_initialized_target() {
    let mut fixture = Fixture::new();
    let before = fixture.observed.clone();
    fixture.observed["api_url"] = json!("http://127.0.0.1:19201");
    assert!(fixture.check().is_err());
    fixture.observed = before.clone();
    fixture.observed["storage"]["sha256"] = json!("b".repeat(64));
    assert!(fixture.check().is_err());
    fixture.observed = before;
    fixture.observed["storage"]["identity"]["executable"] = json!(fixture.second);
    assert!(fixture.check().is_err());
}

#[test]
fn missing_fields_and_relative_executable_fail_closed_without_echoing_values() {
    let mut fixture = Fixture::new();
    fixture.observed["api_url"] = json!({"private":"never-echo"});
    let error = fixture.check().unwrap_err().to_string();
    assert!(!error.contains("never-echo"));
    assert!(executable(&json!("relative/rustfs.exe")).is_err());
}

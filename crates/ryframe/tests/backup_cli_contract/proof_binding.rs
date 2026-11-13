use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use ryframe_kernel::AppResult;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::proof_binding::{GitSource, VerifierBinding};

const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const EXECUTABLE: &[u8] = b"fixture-native-verifier";

struct Fixture {
    backend: tempfile::TempDir,
    coordinator: tempfile::TempDir,
    source: PathBuf,
    grant: PathBuf,
    tool: PathBuf,
    owner: File,
}

impl Fixture {
    fn new() -> Self {
        let backend = tempfile::tempdir().unwrap();
        let build = backend.path().join(".local-tests").join("build");
        fs::create_dir_all(&build).unwrap();
        let source = build.join("source-commit");
        fs::write(&source, format!("{SHA}\n")).unwrap();
        let mut owner = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(build.join("source-commit.lock"))
            .unwrap();
        owner.write_all(b"fixture-owner\n").unwrap();
        let tool = backend.path().join("xtask.exe");
        fs::write(&tool, EXECUTABLE).unwrap();
        let tool = canonical(&tool);
        let grant = build.join("restore-proof-verifier.json");
        let fixture = Self {
            backend,
            coordinator: tempfile::tempdir().unwrap(),
            source,
            grant,
            tool,
            owner,
        };
        fixture.write_grant(&fixture.expected());
        fixture
    }

    fn expected(&self) -> Value {
        json!({
            "format_version": 1,
            "kind": "ryframe-restore-proof-verifier",
            "source_sha": SHA,
            "coordinator": self.coordinator(),
            "verifier": {
                "path": self.tool,
                "bytes": EXECUTABLE.len(),
                "sha256": hex::encode(Sha256::digest(EXECUTABLE)),
            },
        })
    }

    fn write_grant(&self, grant: &Value) {
        fs::write(&self.grant, serde_json::to_vec(grant).unwrap()).unwrap();
    }

    fn read(&self) -> AppResult<VerifierBinding> {
        VerifierBinding::read(&canonical(self.backend.path()), SHA, &self.coordinator())
    }

    fn coordinator(&self) -> GitSource {
        GitSource {
            root: canonical(self.coordinator.path()).to_str().unwrap().into(),
            sha: "b".repeat(40),
        }
    }
}

fn canonical(path: &Path) -> PathBuf {
    let path = path.canonicalize().unwrap();
    #[cfg(windows)]
    let path = PathBuf::from(path.to_str().unwrap().strip_prefix(r"\\?\").unwrap());
    path
}

#[test]
fn reads_the_grant_while_an_independent_file_handle_holds_the_lock() {
    let fixture = Fixture::new();
    fixture.owner.try_lock().unwrap();
    let binding = fixture.read().unwrap();
    assert_eq!(Path::new(&binding.verifier().path), fixture.tool.as_path());
    assert_eq!(binding.verifier().bytes, EXECUTABLE.len() as u64);
    assert_eq!(
        binding.verifier().sha256,
        hex::encode(Sha256::digest(EXECUTABLE))
    );
    assert_eq!(
        serde_json::to_value(binding.verifier()).unwrap(),
        fixture.expected()["verifier"]
    );
    binding.check().unwrap();
    assert_ne!(fixture.coordinator().sha, SHA);
    assert_ne!(
        Path::new(&fixture.coordinator().root),
        canonical(fixture.backend.path())
    );
}

#[test]
fn rejects_missing_or_changed_coordinator_binding_before_using_the_tool() {
    let fixture = Fixture::new();
    fixture.owner.try_lock().unwrap();
    fixture.read().unwrap();
    for field in ["root", "sha"] {
        let mut grant = fixture.expected();
        grant["coordinator"][field] = if field == "root" {
            json!(canonical(fixture.backend.path()))
        } else {
            json!(SHA)
        };
        fixture.write_grant(&grant);
        assert!(
            fixture
                .read()
                .err()
                .unwrap()
                .to_string()
                .contains("与指定协调来源不同")
        );
    }
    let mut absent = fixture.expected();
    absent.as_object_mut().unwrap().remove("coordinator");
    let mut unknown = fixture.expected();
    unknown["coordinator"]["extra"] = json!("private-sentinel");
    let mut array = fixture.expected();
    array["coordinator"] = json!([fixture.coordinator().root, fixture.coordinator().sha]);
    let serialized = serde_json::to_string(&fixture.expected()).unwrap();
    let duplicate = serialized.replace(
        "\"coordinator\":{",
        "\"coordinator\":{\"sha\":\"duplicate\",",
    );
    for bytes in [
        serde_json::to_string(&absent).unwrap(),
        serde_json::to_string(&unknown).unwrap(),
        serde_json::to_string(&array).unwrap(),
        duplicate,
    ] {
        fs::write(&fixture.grant, bytes).unwrap();
        let error = fixture.read().err().unwrap().to_string();
        assert!(error.contains("严格 JSON"), "{error}");
        assert!(!error.contains("private-sentinel"));
    }
}

#[test]
fn rejects_an_unlocked_or_released_source_lock() {
    let fixture = Fixture::new();
    assert!(fixture.read().is_err());
    fixture.owner.try_lock().unwrap();
    let binding = fixture.read().unwrap();
    fixture.owner.unlock().unwrap();
    assert!(binding.check().is_err());
    fixture.owner.try_lock().unwrap();
}

#[test]
fn rejects_changed_source_and_grant_bytes_after_successful_read() {
    let fixture = Fixture::new();
    fixture.owner.try_lock().unwrap();
    let binding = fixture.read().unwrap();
    let mut equivalent = fs::read(&fixture.grant).unwrap();
    equivalent.push(b'\n');
    fs::write(&fixture.grant, equivalent).unwrap();
    assert!(binding.check().is_err());
    fixture.write_grant(&fixture.expected());
    fs::write(&fixture.source, format!("{SHA}\r\n")).unwrap();
    assert!(binding.check().is_err());
    assert!(fixture.read().is_err());
}

#[test]
fn rejects_a_replacement_grant_with_the_original_bytes() {
    let fixture = Fixture::new();
    fixture.owner.try_lock().unwrap();
    let binding = fixture.read().unwrap();
    let original = fs::read(&fixture.grant).unwrap();
    fs::rename(&fixture.grant, fixture.grant.with_extension("preserved")).unwrap();
    fs::write(&fixture.grant, original).unwrap();
    assert!(binding.check().is_err());
}

#[test]
fn rejects_extra_duplicate_and_overlong_grants_without_echoing_input() {
    let fixture = Fixture::new();
    fixture.owner.try_lock().unwrap();
    let expected = fixture.expected();
    let serialized = serde_json::to_string(&expected).unwrap();
    let duplicate = format!(
        "{{\"format_version\":1,{}",
        serialized.strip_prefix('{').unwrap()
    );
    let mut unknown = expected;
    unknown["unknown"] = json!("private-sentinel");
    for invalid in [
        serde_json::to_string(&unknown).unwrap(),
        duplicate,
        " ".repeat(16 * 1024 + 1),
    ] {
        fs::write(&fixture.grant, invalid).unwrap();
        let error = fixture.read().err().unwrap().to_string();
        assert!(!error.contains("private-sentinel"));
    }
}

#[test]
fn rejects_the_wrong_tool_digest_and_same_length_tool_rewrites() {
    let fixture = Fixture::new();
    fixture.owner.try_lock().unwrap();
    let mut grant = fixture.expected();
    grant["verifier"]["sha256"] = json!("b".repeat(64));
    fixture.write_grant(&grant);
    assert!(fixture.read().is_err());
    fixture.write_grant(&fixture.expected());
    let binding = fixture.read().unwrap();
    fs::write(&fixture.tool, vec![b'x'; EXECUTABLE.len()]).unwrap();
    assert!(binding.check().is_err());
}

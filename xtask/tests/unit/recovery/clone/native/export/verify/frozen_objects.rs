use std::fs;

use serde_json::json;

use super::*;

const SCOPE: &str = "fixture-source";
const BUCKET: &str = "uploads";

struct Fixture {
    backend: tempfile::TempDir,
    root: PathBuf,
    export: Value,
    proof_files: Value,
    authority: Value,
}

impl Fixture {
    fn new(capture_scope: &str) -> Self {
        let backend = tempfile::tempdir_in(crate::workspace::root_dir()).unwrap();
        let root = backend.path().join(".local-tests/export");
        let names = evidence::capture_names();
        let mut proof_files = serde_json::Map::new();
        let mut entries = Vec::new();
        let mut digest = Sha256::new();
        digest.update(b"ryframe-copy-saved-object-proofs-v1\n");
        for name in ["z-last", "a-first"] {
            let key = format!("{SCOPE}/{name}");
            let directory = root.join("objects").join(name);
            fs::create_dir_all(&directory).unwrap();
            let artifact = json!({"file":"object.bin","bytes":4,"sha256":"a".repeat(64)});
            let capture = json!({
                "side":"source","scope_id":capture_scope,"bucket":BUCKET,"key":key,
                "metadata":{"ContentType":"text/plain"},"artifact":artifact,
                "get_header_consistent":true,
                "identity":{"ContentLength":4,"ETag":"\"sample\"","VersionId":null},
                "response_sha256":{"head_before":"b".repeat(64)}
            });
            document::write_new(&directory.join("capture.json"), &capture).unwrap();
            for proof in &names {
                let path = directory.join(proof);
                if proof != "capture.json" {
                    fs::write(&path, b"{}\n").unwrap();
                }
                let file = super::digest::file(&path).unwrap();
                digest.update(format!("{proof}\t{}\t{}\n", file.bytes, file.sha256));
                proof_files.insert(
                    manifest::path_text(&path, "测试原始证明").unwrap(),
                    json!({"bytes":file.bytes,"sha256":file.sha256}),
                );
            }
            let file = super::digest::file(&directory.join("capture.json")).unwrap();
            entries.push(json!({
                "key":key,"artifact":artifact,"metadata":capture["metadata"],"get_header_consistent":true,
                "capture":{"file":format!("objects/{name}/capture.json"),"bytes":file.bytes,"sha256":file.sha256}
            }));
        }
        let buckets = BUCKETS.iter().map(|&bucket| {
            json!({"bucket":bucket,"entries":if bucket == BUCKET {entries.as_slice()} else {&[]}})
        }).collect::<Vec<_>>();
        Self {
            backend,
            root,
            export: json!({"scope_id":SCOPE,"objects":buckets}),
            proof_files: Value::Object(proof_files),
            authority: json!({"proofs_sha256":hex::encode(digest.finalize())}),
        }
    }

    fn verify(&self) -> Result<Verified> {
        verify(
            self.backend.path(),
            &self.root,
            &self.export,
            &self.proof_files,
            &self.authority,
        )
    }
}

#[test]
fn captures_are_projected_in_bucket_and_key_order_without_object_payload() {
    let fixture = Fixture::new(SCOPE);
    let captures = fixture.verify().unwrap().finish().unwrap();
    assert_eq!(
        captures.keys().map(String::as_str).collect::<Vec<_>>(),
        ["avatar", "config-packages", "exports", "imports", "uploads"]
    );
    assert!(captures["avatar"].is_empty());
    assert_eq!(
        captures[BUCKET]
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["fixture-source/a-first", "fixture-source/z-last"]
    );
    let directory = fixture.root.join("objects/a-first");
    assert_eq!(
        captures[BUCKET]["fixture-source/a-first"],
        document::read(&directory.join("capture.json")).unwrap()
    );
    assert!(!directory.join("object.bin").exists());
}

#[test]
fn capture_binds_original_bytes_and_uses_the_authority_digest() {
    let mut fixture = Fixture::new(SCOPE);
    let authority = fixture.authority.clone();
    fixture.authority["proofs_sha256"] = json!("f".repeat(64));
    assert!(fixture.verify().is_err());
    fixture.authority = authority;
    let capture = fixture.root.join("objects/a-first/capture.json");
    let original = fs::read(&capture).unwrap();
    let altered = String::from_utf8(original.clone())
        .unwrap()
        .replace("sample", "change");
    assert_eq!(original.len(), altered.len());
    fs::write(&capture, altered).unwrap();
    assert!(fixture.verify().is_err());
}

#[test]
fn inconsistent_capture_identity_and_incomplete_bucket_set_are_rejected() {
    let mut wrong = Fixture::new("fixture-other");
    assert!(wrong.verify().is_err());
    wrong = Fixture::new(SCOPE);
    wrong.export["objects"].as_array_mut().unwrap().pop();
    assert!(wrong.verify().is_err());
}

#[test]
fn failure_marker_created_after_projection_prevents_returning_captures() {
    let fixture = Fixture::new(SCOPE);
    let verified = fixture.verify().unwrap();
    let failure = fixture.root.join("objects/a-first/failure.json");
    fs::write(&failure, b"{}\n").unwrap();
    assert!(verified.finish().is_err());
}

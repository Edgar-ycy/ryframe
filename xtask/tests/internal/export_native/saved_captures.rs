use std::{
    io::Write,
    process::{Output, Stdio},
};

use super::*;
use crate::recovery::clone::native::{
    export::input::public_request,
    s3::{Operation, Request},
};

const LANGUAGE: &str = "en-US";
const BUCKETS: [&str; 5] = ["avatar", "config-packages", "exports", "imports", "uploads"];
const RECOMPUTE: &str = "对象采集结果与原始 HEAD、GET、owner 或实际文件重算不符";
const IDENTITY: &str = "对象身份、完整字节、摘要或采集前后 HEAD 不同";

struct HeadMetadata(FakeS3);

impl Transport for HeadMetadata {
    fn run(&self, spec: CommandSpec) -> crate::Result<CommandOutput> {
        self.0.run(spec)
    }

    fn s3(&self, request: Request) -> crate::Result<Value> {
        let is_head = matches!(request.operation, Operation::Head { .. });
        let mut response = self.0.s3(request)?;
        if is_head {
            response["ContentLanguage"] = json!(LANGUAGE);
        }
        Ok(response)
    }
}

struct SavedCapture {
    fixture: Fixture,
    root: PathBuf,
    receipt: PathBuf,
    export: Value,
}

impl SavedCapture {
    fn new() -> Self {
        let fixture = Fixture::new();
        let context = object_capture_context(&fixture);
        let root = context.output_dir.clone();
        let inventory = json!({"objects":BUCKETS.map(|bucket| {
            let entries = if bucket == "uploads" {
                json!([{"key":"fixture-source/file.txt","bytes":4,"sha256":sha(b"data")}])
            } else {
                json!([])
            };
            json!({"bucket":bucket,"prefix":"fixture-source/","entries":entries})
        })});
        let max_bytes = context.request["max_object_bytes"].as_u64().unwrap();
        let (objects, _) = object::capture(
            &context,
            &HeadMetadata(FakeS3 { wrong_body: false }),
            &inventory,
            max_bytes,
        )
        .unwrap();
        let public = public_request(&fixture.backend, &context.request).unwrap();
        write_json(&root.join("request.json"), &public);
        write_json(&root.join("inventory-before.json"), &inventory);
        let export = json!({
            "format_version":1,"kind":"devex-clone-source-export","status":"source_export_captured",
            "id":context.request["id"],"scope_id":context.request["source"]["scope_id"],
            "request":binding(&context.request_path),
            "source_snapshot":{"head":"a".repeat(40),"clean":true},"worktree_fingerprint":context.request["worktree_fingerprint"],
            "source":public["source"],"artifact_root":root,"databases":[],"objects":objects,
            "enabled_system_schedule_rows":[],"logical_inventory_sha256":"a".repeat(64),
            "evidence":{"request":relative_binding(&root,"request.json"),
                "inventory-before":relative_binding(&root,"inventory-before.json")},
            "catalog_sha256":"b".repeat(64),"quiesced_at":"2026-09-04T00:00:00+00:00",
            "completed_at":"2026-09-04T00:00:01+00:00","requires_offline_plan":true,
            "operator_declared_producers_only":true,"remote_writes":0,"clone_verified":false,
            "restore_qualified":false,"dump_semantic_digest_verified":false,
            "dump_validation":"仅使用固定对象采集输入的测试夹具",
        });
        let receipt = root.join(only_object(&export)["capture"]["file"].as_str().unwrap());
        write_json(&root.join("export.json"), &export);
        Self {
            fixture,
            root,
            receipt,
            export,
        }
    }

    fn invoke(&self) -> (Output, Value) {
        let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
            .arg("__source-evidence")
            .current_dir(&self.fixture.backend)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let request = json!({"operation":"copy-saved-object-captures","write":false,"arguments":{
            "backend":self.fixture.backend,"export":binding(&self.root.join("export.json"))}});
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(&request).unwrap())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.stderr.is_empty());
        let value = serde_json::from_slice(&output.stdout).unwrap();
        (output, value)
    }
}

fn relative_binding(root: &Path, relative: &str) -> Value {
    let mut result = binding(&root.join(relative));
    result.as_object_mut().unwrap().remove("path");
    result["file"] = json!(relative);
    result
}

fn only_object(export: &Value) -> &Value {
    &export["objects"][4]["entries"][0]
}

fn only_object_mut(export: &mut Value) -> &mut Value {
    &mut export["objects"][4]["entries"][0]
}

fn expected_proofs(directory: &Path) -> String {
    let files = snapshot(directory);
    assert_eq!(files.len(), 39);
    let mut digest = Sha256::new();
    digest.update(b"ryframe-copy-saved-object-proofs-v1\n");
    for (path, bytes) in files {
        let name = path.file_name().unwrap().to_str().unwrap();
        if name != "object.bin" {
            digest.update(format!("{name}\t{}\t{}\n", bytes.len(), sha(&bytes)));
        }
    }
    hex::encode(digest.finalize())
}

#[test]
fn native_saved_object_capture_retains_head_only_metadata_and_returns_bounded_offline_proof() {
    let capture = SavedCapture::new();
    let value = native::json::read(&capture.receipt).unwrap();
    assert_eq!(value["get_header_consistent"], false);
    assert_eq!(
        value["get_header_differences"],
        json!([{
            "field":"ContentLanguage","head_present":true,"head_value":LANGUAGE,
            "get_present":false,"get_value":null
        }])
    );
    assert_eq!(value["metadata"]["ContentLanguage"], LANGUAGE);
    let entry = only_object(&capture.export);
    assert_eq!(entry["metadata"], value["metadata"]);
    assert_eq!(entry["get_header_consistent"], false);
    let before = snapshot(&capture.fixture.local);
    let (output, actual) = capture.invoke();
    assert!(output.status.success(), "{actual}");
    assert_eq!(
        actual,
        json!({"ok":true,"result":{"value":{
            "proofs_sha256":expected_proofs(capture.receipt.parent().unwrap())
        }}})
    );
    assert!(output.stdout.len() <= 128);
    assert!(snapshot(&capture.fixture.local) == before);
}

enum Drift {
    Field(&'static str, Value),
    Bytes,
    Missing,
    Failure,
    Export,
}

fn mutate(path: &Path, before: Option<&[u8]>, drift: Drift, capture: &SavedCapture) {
    match drift {
        Drift::Field(name, replacement) => {
            let mut value: Value = serde_json::from_slice(before.unwrap()).unwrap();
            value[name] = replacement;
            write_json(path, &value);
            if path == capture.receipt {
                let mut export = capture.export.clone();
                let relative = only_object(&export)["capture"]["file"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                only_object_mut(&mut export)["capture"] =
                    relative_binding(&capture.root, &relative);
                write_json(&capture.root.join("export.json"), &export);
            }
        }
        Drift::Bytes => {
            let mut bytes = before.unwrap().to_vec();
            bytes[0] ^= 1;
            fs::write(path, bytes).unwrap();
        }
        Drift::Missing => fs::remove_file(path).unwrap(),
        Drift::Failure => write_json(path, &json!({"status":"failed"})),
        Drift::Export => {
            let mut export = capture.export.clone();
            only_object_mut(&mut export)["metadata"]["ContentLanguage"] = json!("fr");
            write_json(path, &export);
        }
    }
}

#[test]
fn native_saved_object_capture_rejects_original_evidence_drift_without_writes() {
    let capture = SavedCapture::new();
    let baseline = snapshot(&capture.fixture.local);
    let directory = capture.receipt.parent().unwrap();
    let export = capture.root.join("export.json");
    let export_bytes = fs::read(&export).unwrap();
    for (name, change, expected) in [
        (
            "intent.json",
            Drift::Field("max_bytes", json!(2048)),
            "对象采集 intent 与明确请求",
        ),
        (
            "capture.json",
            Drift::Field("get_header_consistent", json!(true)),
            RECOMPUTE,
        ),
        (
            "head-before.json",
            Drift::Field("ContentLanguage", json!("fr")),
            IDENTITY,
        ),
        (
            "get.json",
            Drift::Field("ContentLanguage", json!(LANGUAGE)),
            RECOMPUTE,
        ),
        (
            "head-after.json",
            Drift::Field("ContentLanguage", json!("fr")),
            IDENTITY,
        ),
        (
            "get.diagnostic.json",
            Drift::Field("returncode", json!(1)),
            "对象只读请求诊断不完整或包含失败",
        ),
        ("object.bin", Drift::Bytes, IDENTITY),
        (
            "owner-after-uploads.bin",
            Drift::Bytes,
            "对象 ownership 不属于明确计划 scope",
        ),
        ("owner-before-config-packages.json", Drift::Missing, ""),
        ("failure.json", Drift::Failure, "对象采集目录有失败证据"),
        (
            "export.json",
            Drift::Export,
            "完整存储元数据或 GET 差异标记与源证据不同",
        ),
    ] {
        let path = if matches!(change, Drift::Export) {
            export.clone()
        } else {
            directory.join(name)
        };
        let before = if matches!(change, Drift::Failure) {
            assert!(!path.exists());
            None
        } else {
            Some(fs::read(&path).unwrap())
        };
        mutate(&path, before.as_deref(), change, &capture);
        assert_ne!(fs::read(&path).ok(), before, "{name}");
        let changed = snapshot(&capture.fixture.local);

        let (output, actual) = capture.invoke();
        assert!(!output.status.success(), "{name}: {actual}");
        assert_eq!(actual["ok"], false, "{name}");
        let error = &actual["error"];
        assert_eq!(error["category"], "SourceEvidenceRejected", "{name}");
        assert_eq!(error["remote_write_may_have_occurred"], false, "{name}");
        assert_eq!(error["automatic_retry_permitted"], false, "{name}");
        let message = error["message"].as_str().unwrap();
        if expected.is_empty() {
            assert!(!path.exists(), "{name}");
        } else {
            assert!(message.contains(expected), "{name}: {message}");
        }
        assert!(snapshot(&capture.fixture.local) == changed, "{name}");
        if let Some(before) = before {
            fs::write(&path, before).unwrap();
        } else {
            fs::remove_file(&path).unwrap();
        }
        if path == capture.receipt {
            fs::write(&export, &export_bytes).unwrap();
        }
        assert!(snapshot(&capture.fixture.local) == baseline, "{name}");
    }
}

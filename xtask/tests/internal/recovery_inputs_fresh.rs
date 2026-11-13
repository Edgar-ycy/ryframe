use std::{fs, path::PathBuf};

use serde_json::{Value, json};

use super::recovery::inputs::reference;
use super::recovery_inputs_fixture::Fixture;
use super::recovery_inputs_support::{binding, read_value};

#[test]
fn fresh_verify_requires_a_bound_complete_read_only_before_image() {
    let fixture = Fixture::new();
    let original = read_value(&fixture.fresh_verify);
    let mut options = fixture.reference_options(None);
    options.work_dir = fixture.work_dir.with_file_name("fresh-verify-preview");
    let read = || reference::build(&fixture.backend, &options);
    assert_eq!(read().unwrap()["target"], fixture.reference_value["target"]);

    for (pointer, replacement, diagnostic) in [
        ("/status", json!("failed"), "完整前像"),
        ("/remote_writes", json!(true), "远端写入"),
        ("/remote_writes", json!(1), "完整前像"),
        ("/restore_qualified", json!(true), "完整前像"),
        ("/initialized/sha256", json!("f".repeat(64)), "完整前像"),
        ("/inventory/observations", json!({}), "完整前像"),
    ] {
        let mut verify = original.clone();
        *verify.pointer_mut(pointer).unwrap() = replacement;
        fs::write(&fixture.fresh_verify, serde_json::to_vec(&verify).unwrap()).unwrap();
        let error = read().unwrap_err().to_string();
        assert!(error.contains(diagnostic), "{pointer}: {error}");
    }
    for pointer in ["", "/inventory"] {
        let mut verify = original.clone();
        verify
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("extra".into(), Value::Bool(true));
        fs::write(&fixture.fresh_verify, serde_json::to_vec(&verify).unwrap()).unwrap();
        let error = read().unwrap_err().to_string();
        assert!(error.contains("未知字段"), "{pointer}: {error}");
    }
}

#[test]
fn fresh_verify_matches_inventory_logical_keys_to_physical_identities() {
    let fixture = Fixture::new();
    let original = read_value(&fixture.fresh_verify);
    let path = file_path(&original["inventory"]["receipt"]);
    let receipt = read_value(&path);
    let mut options = fixture.reference_options(None);
    options.work_dir = fixture.work_dir.with_file_name("fresh-verify-preview");
    let read = || reference::build(&fixture.backend, &options);
    assert_ne!(
        receipt["observations"],
        original["inventory"]["observations"]
    );
    assert_eq!(read().unwrap()["target"], fixture.reference_value["target"]);
    for (pointer, replacement, diagnostic) in [
        ("/status", json!("failed"), "本侧四个目标"),
        ("/scope_id", json!("other"), "本侧四个目标"),
        (
            "/observations/shared-control/resource/database",
            json!("other_db"),
            "物理身份不同",
        ),
        (
            "/observations/shared-control/resource/server_uuid",
            json!("other-server"),
            "物理身份不同",
        ),
        (
            "/observations/shared-control/ownership/0/scope_id",
            json!("other"),
            "原实际采集收据不同",
        ),
        ("/remote_writes", json!(1), "必须保持只读"),
    ] {
        let mut changed = receipt.clone();
        *changed.pointer_mut(pointer).unwrap() = replacement;
        fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
        let mut verify = original.clone();
        verify["inventory"]["receipt"] = binding(&path);
        fs::write(&fixture.fresh_verify, serde_json::to_vec(&verify).unwrap()).unwrap();
        let error = read().unwrap_err().to_string();
        assert!(error.contains(diagnostic), "{pointer}: {error}");
    }
}

#[test]
fn fresh_verify_rejects_rebound_initial_ownership_from_another_resource() {
    for (pointer, replacement, diagnostic) in [
        (
            "/objects/uploads/owner/sha256",
            json!("e".repeat(64)),
            "对象 ownership",
        ),
        (
            "/objects/uploads/keys",
            json!(["base-a/.ryframe-owner", "base-a/unknown"]),
            "对象 ownership",
        ),
        (
            "/inventory/observations/base_shared_control/ownership/0/scope_id",
            json!("other"),
            "数据库 ownership",
        ),
    ] {
        let fixture = Fixture::new();
        let mut options = fixture.reference_options(None);
        options.work_dir = fixture.work_dir.with_file_name("fresh-verify-preview");
        let mut arm = read_value(&fixture.arm_input);
        let initialized_path = file_path(&arm["initialized"]);
        let mut initialized = read_value(&initialized_path);
        *initialized.pointer_mut(pointer).unwrap() = replacement;
        fs::write(&initialized_path, serde_json::to_vec(&initialized).unwrap()).unwrap();
        arm["initialized"] = binding(&initialized_path);
        for field in ["request", "manifest"] {
            let path = file_path(&arm[field]);
            let mut value = read_value(&path);
            value["initialized"] = arm["initialized"].clone();
            fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            arm[field] = binding(&path);
        }
        fs::write(&fixture.arm_input, serde_json::to_vec(&arm).unwrap()).unwrap();

        let mut verify = read_value(&fixture.fresh_verify);
        verify["initialized"] = arm["initialized"].clone();
        verify["inventory"]["observations"] = initialized["inventory"]["observations"].clone();
        let receipt_path = file_path(&verify["inventory"]["receipt"]);
        let mut receipt = read_value(&receipt_path);
        receipt["observations"]["shared-control"] =
            verify["inventory"]["observations"]["base_shared_control"].clone();
        fs::write(&receipt_path, serde_json::to_vec(&receipt).unwrap()).unwrap();
        verify["inventory"]["receipt"] = binding(&receipt_path);
        fs::write(&fixture.fresh_verify, serde_json::to_vec(&verify).unwrap()).unwrap();
        let error = reference::build(&fixture.backend, &options)
            .unwrap_err()
            .to_string();
        assert!(error.contains(diagnostic), "{pointer}: {error}");
    }
}

fn file_path(descriptor: &Value) -> PathBuf {
    PathBuf::from(descriptor["path"].as_str().unwrap())
}

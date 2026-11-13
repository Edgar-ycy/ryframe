use std::{fs, path::PathBuf};

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde_json::{Value, json};

use super::recovery::inputs::{backup, document};
use super::recovery_inputs_fixture::Fixture;
use super::recovery_inputs_support::{binding, hash, read_value};

#[test]
fn backup_receipt_requires_the_original_completed_result_and_manifest() {
    let fixture = Fixture::new();
    let original = read_value(&fixture.backup_receipt);
    assert_eq!(verify(&fixture).unwrap().result, original["result"]);
    for (pointer, replacement, diagnostic) in [
        ("/status", json!("failed"), "已完成正式备份"),
        ("/command", json!("copy"), "已完成正式备份"),
        ("/plan_sha256", json!("f".repeat(64)), "已完成正式备份"),
        ("/result/artifacts", json!(1), "产物数量"),
        (
            "/result/manifest/sha256",
            json!("f".repeat(64)),
            "正式备份清单与登记摘要",
        ),
    ] {
        let mut receipt = original.clone();
        *receipt.pointer_mut(pointer).unwrap() = replacement;
        write(&fixture.backup_receipt, &receipt);
        let error = verify(&fixture).err().unwrap().to_string();
        assert!(error.contains(diagnostic), "{pointer}: {error}");
    }
    for (pointer, diagnostic) in [("", "备份收据"), ("/result", "正式备份结果")] {
        let mut receipt = original.clone();
        receipt
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("extra".into(), json!(true));
        write(&fixture.backup_receipt, &receipt);
        let error = verify(&fixture).err().unwrap().to_string();
        assert!(
            error.contains(diagnostic) && error.contains("未知字段"),
            "{pointer}: {error}"
        );
    }
    let mut receipt = original.clone();
    receipt["result"]
        .as_object_mut()
        .unwrap()
        .remove("source_export");
    write(&fixture.backup_receipt, &receipt);
    assert!(
        verify(&fixture)
            .err()
            .unwrap()
            .to_string()
            .contains("正式备份结果字段缺失")
    );

    let mut changed_reference = original;
    changed_reference["result"]["reference_plan"]["source"]["scope_id"] = json!("other-scope");
    changed_reference["plan_sha256"] = json!(hash(&changed_reference["result"]["reference_plan"]));
    write(&fixture.backup_receipt, &changed_reference);
    assert!(
        verify(&fixture)
            .err()
            .unwrap()
            .to_string()
            .contains("已完成正式备份")
    );
}

#[test]
fn backup_receipt_checks_manifest_completion_and_retention_times() {
    let fixture = Fixture::new();
    let original = read_value(&fixture.backup_receipt);
    let path = PathBuf::from(original["result"]["manifest"]["path"].as_str().unwrap());
    let manifest = read_value(&path);
    let parse = |value: &Value| DateTime::parse_from_rfc3339(value.as_str().unwrap()).unwrap();
    let captured = parse(&manifest["captured_at"]);
    let finished = parse(&original["completed_at"]);
    for (pointer, replacement, diagnostic) in [
        ("/started_at", json!("invalid"), "备份收据开始时间"),
        (
            "/completed_at",
            json!(utc(captured)),
            "备份收据完成时间早于开始时间",
        ),
    ] {
        let mut receipt = original.clone();
        *receipt.pointer_mut(pointer).unwrap() = replacement;
        write(&fixture.backup_receipt, &receipt);
        let error = verify(&fixture).err().unwrap().to_string();
        assert!(error.contains(diagnostic), "{pointer}: {error}");
    }
    for (field, replacement, diagnostic) in [
        (
            "completed_at",
            json!(utc(captured - Duration::microseconds(1))),
            "采集、完成及收据时间",
        ),
        (
            "completed_at",
            json!(utc(finished + Duration::microseconds(1))),
            "采集、完成及收据时间",
        ),
        (
            "completed_at",
            json!(utc(finished + Duration::nanoseconds(1))),
            "备份清单时间必须符合微秒精度",
        ),
        ("retention_until", json!("invalid"), "备份清单保留时间"),
        (
            "retention_until",
            json!(utc(captured + Duration::days(7) - Duration::microseconds(1))),
            "至少保留采集后的 7 天",
        ),
    ] {
        let mut changed = manifest.clone();
        changed[field] = replacement;
        write(&path, &changed);
        let mut receipt = original.clone();
        receipt["result"]["manifest"] = binding(&path);
        write(&fixture.backup_receipt, &receipt);
        let error = verify(&fixture).err().unwrap().to_string();
        assert!(error.contains(diagnostic), "{field}: {error}");
    }

    let mut equal = manifest;
    equal["completed_at"] = original["completed_at"].clone();
    equal["retention_until"] = json!(utc(captured + Duration::days(7)));
    write(&path, &equal);
    let mut receipt = original;
    receipt["result"]["manifest"] = binding(&path);
    write(&fixture.backup_receipt, &receipt);
    assert_eq!(verify(&fixture).unwrap().manifest, equal);

    // 清单完成时间仅保存微秒，开始收据允许保存同一微秒内的纳秒。
    receipt["started_at"] = json!(utc(finished + Duration::nanoseconds(1)));
    receipt["completed_at"] = json!(utc(finished + Duration::nanoseconds(2)));
    write(&fixture.backup_receipt, &receipt);
    assert_eq!(verify(&fixture).unwrap().manifest, equal);
}

fn verify(fixture: &Fixture) -> crate::Result<backup::Backup> {
    let receipt = document::read(&fixture.backup_receipt, &fixture.backend, "备份收据")?;
    backup::validate(&fixture.backend, &fixture.reference_value, &receipt)
}

fn write(path: &std::path::Path, value: &Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn utc(value: DateTime<chrono::FixedOffset>) -> String {
    value
        .with_timezone(&Utc)
        .to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

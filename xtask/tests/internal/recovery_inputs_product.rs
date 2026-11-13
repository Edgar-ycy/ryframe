use std::{fs, path::PathBuf};

use chrono::{DateTime, Duration, SecondsFormat, Timelike, Utc};
use serde_json::{Value, json};

use super::recovery_inputs_fixture::Fixture;
use super::recovery_inputs_support::{binding, capture_target_plan, read_value};

#[test]
fn saved_product_plan_rejects_substituted_fields_and_database_order() {
    let fixture = Fixture::new();
    assert_eq!(
        capture(&fixture, &fixture.product_value).unwrap()["product_plan"],
        fixture.product_value
    );
    let mut reversed = fixture.product_value["databases"]
        .as_array()
        .unwrap()
        .clone();
    reversed.reverse();
    for (pointer, value, diagnostic) in [
        ("/frontend_sha", json!("e".repeat(40)), "frontend_sha"),
        ("/backup_id", json!("other"), "backup_id"),
        ("/scope_id", json!("other"), "scope_id"),
        ("/object_prefix", json!("source-a/"), "object_prefix"),
        (
            "/object_endpoint",
            json!("http://127.0.0.1:19001"),
            "object_endpoint",
        ),
        (
            "/worker_ready_url",
            fixture.product_value["api_ready_url"].clone(),
            "worker_ready_url",
        ),
        (
            "/api_ready_url",
            json!("http://127.0.0.1:18888/readyz"),
            "api_ready_url",
        ),
        ("/databases/0/database", json!("other"), "databases"),
        (
            "/databases/0/server_uuid",
            json!("other-server"),
            "databases",
        ),
        ("/databases/0/target_key", json!("other"), "databases"),
        ("/databases", json!(reversed), "databases"),
    ] {
        let mut plan = fixture.product_value.clone();
        *plan.pointer_mut(pointer).unwrap() = value;
        let error = capture(&fixture, &plan).unwrap_err().to_string();
        assert!(error.contains(diagnostic), "{pointer}: {error}");
    }
    for (pointer, label) in [("", "产品恢复计划"), ("/databases/0", "产品数据库")] {
        let mut plan = fixture.product_value.clone();
        plan.pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("extra".into(), json!(true));
        let error = capture(&fixture, &plan).unwrap_err().to_string();
        assert!(
            error.contains(label) && error.contains("未知字段"),
            "{pointer}: {error}"
        );
    }
}

#[test]
fn saved_product_plan_requires_the_exact_serialized_utc_fault_time() {
    let fixture = Fixture::new();
    let mut plan = fixture.product_value.clone();
    let anchor = DateTime::parse_from_rfc3339(plan["fault_at"].as_str().unwrap())
        .unwrap()
        .with_timezone(&Utc)
        - Duration::seconds(1);
    let whole = anchor.with_nanosecond(0).unwrap();
    let millis = whole + Duration::milliseconds(100);
    let micros = whole + Duration::microseconds(123_450);
    for (fault, precision) in [
        (whole, SecondsFormat::Secs),
        (millis, SecondsFormat::Millis),
        (micros, SecondsFormat::Micros),
    ] {
        let encoded = json!(fault.to_rfc3339_opts(precision, true));
        plan["fault_at"] = encoded.clone();
        let output = capture(&fixture, &plan).unwrap();
        assert_eq!(output["product_plan"]["fault_at"], encoded);
    }

    let second = whole.format("%Y-%m-%dT%H:%M:%S");
    for (fault, diagnostic) in [
        (whole.to_rfc3339_opts(SecondsFormat::Secs, false), "UTC Z"),
        (format!("{second}.1Z"), "Rust 原生"),
        (format!("{second}.100000Z"), "Rust 原生"),
        (format!("{second}.000Z"), "Rust 原生"),
        (format!("{second}.1234567Z"), "Rust 原生"),
        ("today".into(), "UTC Z"),
        ("2026-02-30T00:00:00Z".into(), "不是有效 UTC 时间"),
    ] {
        plan["fault_at"] = json!(fault);
        let error = capture(&fixture, &plan).unwrap_err().to_string();
        assert!(error.contains(diagnostic), "{fault}: {error}");
    }
}

#[test]
fn saved_product_plan_rechecks_the_exact_recovery_point() {
    let fixture = Fixture::new();
    let mut receipt = read_value(&fixture.backup_receipt);
    let manifest_path = PathBuf::from(receipt["result"]["manifest"]["path"].as_str().unwrap());
    let mut manifest = read_value(&manifest_path);
    let original = DateTime::parse_from_rfc3339(manifest["captured_at"].as_str().unwrap())
        .unwrap()
        .with_timezone(&Utc);
    // 两个端点均落在过去，单独区分备份采集与 24 小时边界。
    let captured = original - Duration::hours(25);
    manifest["captured_at"] = json!(captured.to_rfc3339_opts(SecondsFormat::AutoSi, true));
    manifest["quiesced_at"] = json!(captured.to_rfc3339_opts(SecondsFormat::AutoSi, true));
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    receipt["result"]["manifest"] = binding(&manifest_path);
    fs::write(
        &fixture.backup_receipt,
        serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .unwrap();

    let boundary = captured + Duration::hours(24);
    let mut plan = fixture.product_value.clone();
    for valid in [captured, boundary] {
        plan["fault_at"] = json!(valid.to_rfc3339_opts(SecondsFormat::AutoSi, true));
        assert_eq!(capture(&fixture, &plan).unwrap()["product_plan"], plan);
    }
    for invalid in [
        captured - Duration::microseconds(1),
        boundary + Duration::microseconds(1),
    ] {
        plan["fault_at"] = json!(invalid.to_rfc3339_opts(SecondsFormat::AutoSi, true));
        assert!(
            capture(&fixture, &plan)
                .unwrap_err()
                .to_string()
                .contains("24 小时")
        );
    }
    manifest["captured_at"] = json!(original.to_rfc3339_opts(SecondsFormat::AutoSi, true));
    manifest["quiesced_at"] = json!(original.to_rfc3339_opts(SecondsFormat::AutoSi, true));
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    receipt["result"]["manifest"] = binding(&manifest_path);
    fs::write(
        &fixture.backup_receipt,
        serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .unwrap();
    plan["fault_at"] =
        json!((Utc::now() + Duration::minutes(1)).to_rfc3339_opts(SecondsFormat::Secs, true));
    assert!(
        capture(&fixture, &plan)
            .unwrap_err()
            .to_string()
            .contains("未来")
    );
}

fn capture(fixture: &Fixture, value: &Value) -> crate::Result<Value> {
    fs::write(&fixture.product_plan, serde_json::to_vec_pretty(value)?).unwrap();
    capture_target_plan(
        &fixture.backend,
        &fixture.reference_plan,
        &fixture.backup_receipt,
        &fixture.comparison_sources,
        &fixture.arm_input,
        &fixture.fresh_verify,
        &fixture.product_plan,
    )
}

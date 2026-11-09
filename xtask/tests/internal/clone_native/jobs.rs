use super::*;

fn cleanup(fixture: &mut Fixture, index: usize, handler: &str, job_type: &str) {
    let schedule_id = 1 + index as i64;
    let job_id = 99_256_017_304_428_544_i64 + index as i64;
    let scheduled = "2026-09-01 00:00:00.123456";
    fixture.add(
        "shared-control",
        "sys_job_schedule",
        &[
            ("id", json!(schedule_id)),
            ("tenant_id", json!("system")),
            ("handler_key", json!(handler)),
            ("enabled", json!(0)),
            ("del_flag", json!("0")),
            ("version", json!(1)),
            ("created_at", json!(scheduled)),
            ("updated_at", json!(scheduled)),
        ],
    );
    fixture.add(
        "shared-control",
        "sys_background_job",
        &[
            ("id", json!(job_id)),
            ("tenant_id", Value::Null),
            ("job_type", json!(job_type)),
            ("status", json!("succeeded")),
            ("completed_at", json!("2026-09-01 00:00:01.000001")),
            ("schedule_id", json!(schedule_id)),
            ("scheduled_for", json!(scheduled)),
            (
                "payload",
                json!({"schedule_id":schedule_id.to_string(),
            "trigger_kind":"scheduled","scheduled_for":"2026-09-01T00:00:00.123456Z"}),
            ),
        ],
    );
    fixture.add(
        "shared-control",
        "sys_job_schedule_execution",
        &[
            ("id", json!(99_256_017_220_542_464_i64 + index as i64)),
            ("tenant_id", json!("system")),
            ("schedule_id", json!(schedule_id)),
            ("background_job_id", json!(job_id)),
            ("outcome", json!("enqueued")),
            ("trigger_kind", json!("scheduled")),
            ("scheduled_for", json!(scheduled)),
        ],
    );
    fixture.refresh();
}

#[test]
fn global_cleanup_schedule_relations_use_product_job_types_and_preserve_boundaries() {
    for (index, (handler, job_type)) in [
        ("system.export_result_cleanup", "system.export.cleanup"),
        (
            "system.message_retention_cleanup",
            "system.message.retention",
        ),
        (
            "system.data_retention_cleanup",
            "system.data_retention.cleanup",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let mut fixture = Fixture::new();
        cleanup(&mut fixture, index, handler, job_type);
        fixture.plan("cleanup-valid.json").unwrap();
        let original = fixture.data.clone();
        for (label, table, field, value) in [
            (
                "job-tenant",
                "sys_background_job",
                "tenant_id",
                json!("system"),
            ),
            (
                "schedule-tenant",
                "sys_job_schedule",
                "tenant_id",
                json!("tenant-a"),
            ),
            (
                "execution-tenant",
                "sys_job_schedule_execution",
                "tenant_id",
                json!("tenant-a"),
            ),
            (
                "wrong-type",
                "sys_background_job",
                "job_type",
                json!("system.data.retention.cleanup"),
            ),
            (
                "parent-schedule",
                "sys_background_job",
                "schedule_id",
                json!(999),
            ),
            (
                "missing-parent",
                "sys_job_schedule_execution",
                "background_job_id",
                json!(999),
            ),
            (
                "payload-trigger",
                "sys_background_job",
                "payload",
                json!({
                    "schedule_id":(1 + index).to_string(),"trigger_kind":"manual",
                    "scheduled_for":"2026-09-01T00:00:00.123456Z"
                }),
            ),
        ] {
            fixture.data = original.clone();
            set_row(&mut fixture, "shared-control", table, field, value);
            fixture.refresh();
            assert!(
                fixture
                    .plan(&format!("cleanup-reject-{label}.json"))
                    .is_err(),
                "{handler}: {label}"
            );
        }
    }
}

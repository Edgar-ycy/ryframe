use chrono::{DateTime, Utc};
use ryframe_application::{
    ScheduledJobContext, ScheduledJobTargetRegistry, ScheduledJobTargetScope,
    maintenance_schedule_targets, message_schedule_targets,
};
use serde_json::json;

#[test]
fn schedule_targets_are_registered_by_domain_groups() {
    let registry = ScheduledJobTargetRegistry::new()
        .with_targets(maintenance_schedule_targets())
        .unwrap()
        .with_targets(message_schedule_targets(false))
        .unwrap();
    let descriptors = registry.descriptors_for_tenant("system");
    assert_eq!(
        descriptors
            .iter()
            .map(|target| target.handler_key.as_str())
            .collect::<Vec<_>>(),
        [
            "system.data_retention_cleanup",
            "system.export_result_cleanup",
            "system.message_retention_cleanup",
        ]
    );
    assert!(
        !descriptors
            .iter()
            .find(|target| target.handler_key == "system.message_retention_cleanup")
            .unwrap()
            .available
    );
}

#[test]
fn grouped_registration_still_rejects_duplicate_targets() {
    let targets = maintenance_schedule_targets();
    let duplicate = targets[0].clone();
    let error = ScheduledJobTargetRegistry::new()
        .with_targets(targets)
        .unwrap()
        .with_targets([duplicate])
        .err()
        .expect("重复目标必须拒绝");
    assert!(error.to_string().contains("调度目标重复注册"));
}

#[test]
fn system_cleanup_targets_build_the_payload_required_by_copy() {
    let registry = ScheduledJobTargetRegistry::new()
        .with_targets(maintenance_schedule_targets())
        .unwrap()
        .with_targets(message_schedule_targets(true))
        .unwrap();
    let schedule_id = 99_256_017_304_428_544;
    let timestamp = "2026-09-01T00:00:00.123456Z";
    let scheduled_for = timestamp.parse::<DateTime<Utc>>().unwrap();
    for (handler, job_type) in [
        ("system.export_result_cleanup", "system.export.cleanup"),
        (
            "system.message_retention_cleanup",
            "system.message.retention",
        ),
        (
            "system.data_retention_cleanup",
            "system.data_retention.cleanup",
        ),
    ] {
        let target = registry.get(handler).unwrap();
        assert_eq!(target.scope(), ScheduledJobTargetScope::System);
        for trigger_kind in ["scheduled", "manual", "misfire"] {
            let job = target
                .build_job(&ScheduledJobContext {
                    tenant_id: "system",
                    schedule_id,
                    trigger_kind,
                    scheduled_for,
                    max_runtime_seconds: 900,
                    fire_key: "fixture-fire",
                })
                .unwrap();
            assert_eq!(job.job_type, job_type);
            assert_eq!(job.tenant_id, None);
            assert_eq!(job.schedule_id, Some(schedule_id));
            assert_eq!(job.scheduled_for, Some(scheduled_for));
            assert_eq!(
                job.payload,
                json!({
                    "schedule_id":schedule_id.to_string(),
                    "trigger_kind":trigger_kind,
                    "scheduled_for":timestamp,
                })
            );
        }
    }
}

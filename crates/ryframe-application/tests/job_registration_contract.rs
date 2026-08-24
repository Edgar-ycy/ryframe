use ryframe_application::{
    ScheduledJobTargetRegistry, maintenance_schedule_targets, message_schedule_targets,
};

#[test]
fn schedule_targets_are_registered_by_domain_groups() {
    let registry = ScheduledJobTargetRegistry::new()
        .with_targets(maintenance_schedule_targets())
        .unwrap()
        .with_targets(message_schedule_targets(false))
        .unwrap();
    let descriptors = registry.descriptors_for_tenant("system");
    assert_eq!(descriptors.len(), 3);
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

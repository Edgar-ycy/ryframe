use crate::{
    recovery::{
        clone::native::{
            bridge, digest, json as document, maintenance,
            resources::common,
            source,
            target_verify::input::request::{self, SourceObserver},
        },
        fixture_control::successor::arm_target::verify_maintenance,
    },
    target_verify_fixture::{Fixture, MaintenanceFixture},
};
use serde_json::json;

#[test]
fn arm_target_accepts_audited_bridge_shape_and_checks_bound_execution_jobs() {
    let fixture = Fixture::new();
    let build = fixture.local.join("maintenance/build.json");
    let mut receipt = document::read(&build).unwrap();
    receipt["toolchain"] =
        maintenance::toolchain(&fixture.backend, &maintenance::DefaultRunner).unwrap();
    document::write_replace(&build, &receipt).unwrap();
    let inventory = fixture.local.join("maintenance-inventory.json");
    document::write_new(
        &inventory,
        &source::inventory_value(&source::capture(&fixture.backend).unwrap()),
    )
    .unwrap();
    let audit = fixture.local.join("maintenance-audit.json");
    bridge::write(&fixture.backend, &build, &inventory, &audit).unwrap();
    let bridges = [json!({"path":audit,"sha256":digest::file(&audit).unwrap().sha256})];
    let target = json!({"maintenance_build":common::binding(&build).unwrap()});
    verify_maintenance(&fixture.backend, &target, &bridges).unwrap();

    let current = ryframe_config::ToolSettings::load(&fixture.backend)
        .unwrap()
        .verification
        .build_jobs;
    let different = if current == 1 { 2 } else { 1 };
    let tools = fixture.backend.join(".local-tests/tools.toml");
    std::fs::write(&tools, format!("[verification]\nbuild_jobs={different}\n")).unwrap();
    let error = verify_maintenance(&fixture.backend, &target, &bridges).unwrap_err();
    assert!(error.to_string().contains("verification.build_jobs"));
}

#[test]
fn arm_target_keeps_source_bridges_in_coordinator_and_checks_its_own_build() {
    let coordinator =
        MaintenanceFixture::new_in(&crate::workspace::root_dir().join(".local-tests"));
    let product = MaintenanceFixture::new_in(&coordinator.local);
    let source_build = coordinator.local.join("maintenance/build.json");
    let source_inventory = coordinator.local.join("maintenance-inventory.json");
    document::write_new(
        &source_inventory,
        &source::inventory_value(&source::capture(&coordinator.backend).unwrap()),
    )
    .unwrap();
    let audit = coordinator.local.join("maintenance-audit.json");
    bridge::write(
        &coordinator.backend,
        &source_build,
        &source_inventory,
        &audit,
    )
    .unwrap();
    let bridges = [json!({"path":audit,"sha256":digest::file(&audit).unwrap().sha256})];

    let build = product.local.join("maintenance/build.json");
    let mut receipt = document::read(&build).unwrap();
    receipt["toolchain"] =
        maintenance::toolchain(&product.backend, &maintenance::DefaultRunner).unwrap();
    document::write_replace(&build, &receipt).unwrap();
    let fixture = coordinator.local.join("product-fixture.json");
    let snapshot = request::SystemSourceObserver
        .snapshot(&product.backend)
        .unwrap();
    document::write_new(
        &fixture,
        &json!({
            "format_version":1,"fixture":"business","status":"ready",
            "paths":{"backend":product.backend},"generated":{"backend":snapshot}
        }),
    )
    .unwrap();
    let mut target = json!({"maintenance_build":common::binding(&build).unwrap(),
        "execution_backend":{"fixture":common::binding(&fixture).unwrap(),"path":product.backend}});
    verify_maintenance(&coordinator.backend, &target, &bridges).unwrap();

    target["maintenance_build"] = common::binding(&source_build).unwrap();
    let error = verify_maintenance(&coordinator.backend, &target, &bridges).unwrap_err();
    assert!(error.to_string().contains("arm 维护构建"), "{error}");

    target["maintenance_build"] = common::binding(&build).unwrap();
    std::fs::write(&audit, b"{}").unwrap();
    let error = verify_maintenance(&coordinator.backend, &target, &bridges).unwrap_err();
    assert!(
        error.to_string().contains("来源审计文件摘要变化"),
        "{error}"
    );
}

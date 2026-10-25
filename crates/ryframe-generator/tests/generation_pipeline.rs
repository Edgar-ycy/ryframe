#[path = "generation_pipeline/conflict_fixture.rs"]
mod conflict_fixture;
#[path = "generation_pipeline/fixture.rs"]
mod fixture;
#[path = "support/resource_workspace.rs"]
mod resource_workspace_control;
#[path = "../src/command/staging/snapshot.rs"]
mod snapshot;

#[test]
#[ignore = "跨仓候选生成需要已安装的前端依赖和真实 Cargo/Node；不访问服务"]
fn two_resources_generate_actual_contract_and_pages_without_partial_installation() {
    let (frontend_dir, _, target_dir) =
        resource_workspace_control::load().expect("跨仓候选生成需要受管资源 Workspace 控制文件");
    fixture::verify(&frontend_dir, &target_dir);
}

#[test]
#[ignore = "跨模块正式入口校验需要受管前端依赖和真实 Cargo；不访问服务"]
fn cross_module_structure_conflicts_reject_before_install_with_or_without_frontend_sync() {
    let (frontend_dir, _, target_dir) =
        resource_workspace_control::load().expect("冲突验证需要受管资源 Workspace 控制文件");
    conflict_fixture::verify_conflicts(
        &frontend_dir,
        &target_dir,
        &[
            ("resource", "业务资源名称冲突"),
            ("table", "业务表名冲突"),
            ("route", "业务路由冲突"),
        ],
    );
}

#[test]
#[ignore = "跨模块权限校验需要受管前端依赖和真实 Cargo；不访问服务"]
fn cross_module_permission_conflict_rejects_before_install_with_or_without_frontend_sync() {
    let (frontend_dir, _, target_dir) =
        resource_workspace_control::load().expect("冲突验证需要受管资源 Workspace 控制文件");
    conflict_fixture::verify_conflicts(
        &frontend_dir,
        &target_dir,
        &[("permission", "业务权限与其他资源或目录冲突")],
    );
}

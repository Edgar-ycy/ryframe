#[path = "generation_pipeline/fixture.rs"]
mod fixture;
#[path = "support/resource_workspace.rs"]
mod resource_workspace_control;

#[test]
#[ignore = "跨仓候选生成需要已安装的前端依赖和真实 Cargo/Node；不访问服务"]
fn two_resources_generate_actual_contract_and_pages_without_partial_installation() {
    let (frontend_dir, _, target_dir) =
        resource_workspace_control::load().expect("跨仓候选生成需要受管资源 Workspace 控制文件");
    fixture::verify(&frontend_dir, &target_dir);
}

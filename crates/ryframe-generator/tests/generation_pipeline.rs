#[path = "generation_pipeline/fixture.rs"]
mod fixture;

#[test]
#[ignore = "跨仓候选生成需要已安装的前端依赖和真实 Cargo/Node；不访问服务"]
fn two_resources_generate_actual_contract_and_pages_without_partial_installation() {
    fixture::verify();
}

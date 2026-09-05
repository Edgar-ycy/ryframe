use std::sync::Once;

use ryframe_application::install_id_generator;
use ryframe_kernel::AppResult;

static TEST_ID_GENERATOR: Once = Once::new();

fn fixed_test_id() -> AppResult<i64> {
    Ok(43)
}

fn ensure_test_id_generator() {
    TEST_ID_GENERATOR.call_once(|| {
        install_id_generator(fixed_test_id).expect("测试 ID 生成器应安装成功");
    });
}

#[path = "application_contracts/export_execution_contract.rs"]
mod export_execution_contract;
#[path = "application_contracts/export_request_contract.rs"]
mod export_request_contract;
#[path = "application_contracts/job_registration_contract.rs"]
mod job_registration_contract;
#[path = "application_contracts/job_wakeup_contract.rs"]
mod job_wakeup_contract;
#[path = "application_contracts/policy_contract.rs"]
mod policy_contract;
#[path = "application_contracts/port_contract.rs"]
mod port_contract;
#[path = "application_contracts/post_export_contract.rs"]
mod post_export_contract;
#[path = "application_contracts/system_memory_contract.rs"]
mod system_memory_contract;
#[path = "application_contracts/system_policy_contract.rs"]
mod system_policy_contract;
#[path = "application_contracts/tenant_provisioning.rs"]
mod tenant_provisioning;
#[path = "application_contracts/transaction_contract.rs"]
mod transaction_contract;
#[path = "application_contracts/user_query_contract.rs"]
mod user_query_contract;

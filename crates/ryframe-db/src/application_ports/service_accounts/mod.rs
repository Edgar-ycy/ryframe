mod audit;
mod authorization;
mod read;
mod write;

use crate::entities::service_account;
use ryframe_application::ports::service_accounts::ServiceAccountRecord;

pub use audit::port as audit;
pub use authorization::port as authorization;
pub use read::port as read;
pub use write::port as write;

fn account_record(account: service_account::Model) -> ServiceAccountRecord {
    ServiceAccountRecord {
        id: account.id,
        tenant_id: account.tenant_id,
        code: account.code,
        name: account.name,
        description: account.description,
        dept_id: account.dept_id,
        status: account.status,
        authorization_version: account.authorization_version,
        max_requests_per_minute: account.max_requests_per_minute,
        created_by: account.created_by,
        deleted: account.del_flag == service_account::Model::DEL_FLAG_DELETED,
        created_at: account.created_at,
        updated_at: account.updated_at,
    }
}

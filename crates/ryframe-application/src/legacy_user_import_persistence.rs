use std::sync::Arc;

use ryframe_db::{ControlDatabaseCluster, DeptRepository};

use crate::{PersistenceFuture, UserImportDepartmentReadPort, UserImportDepartmentRecord};

pub fn department_port(database: ControlDatabaseCluster) -> Arc<dyn UserImportDepartmentReadPort> {
    Arc::new(LegacyUserImportDepartmentRead { database })
}

struct LegacyUserImportDepartmentRead {
    database: ControlDatabaseCluster,
}

impl UserImportDepartmentReadPort for LegacyUserImportDepartmentRead {
    fn list<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, Vec<UserImportDepartmentRecord>> {
        Box::pin(async move {
            DeptRepository
                .find_filtered(self.database.write(), tenant_id, None, None)
                .await
                .map(|departments| {
                    departments
                        .into_iter()
                        .map(|department| UserImportDepartmentRecord {
                            id: department.id,
                            name: department.name,
                            parent_id: department.parent_id,
                            ancestors: department.ancestors,
                            status: department.status,
                        })
                        .collect()
                })
        })
    }
}

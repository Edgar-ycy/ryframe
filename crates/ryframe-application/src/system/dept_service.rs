use std::sync::Arc;

use crate::{AuthorizationCache, DeptReadPort, DeptWritePort};

mod commands;
mod model;
mod queries;

pub use model::{CreateDeptCommand, DeptTreeNode, DeptVo, UpdateDeptCommand};

const CACHE_TTL_SECS: u64 = 3600;
const DEPT_TREE_CACHE_NAMESPACE: &str = "dept-tree";

pub struct DeptService {
    read: Arc<dyn DeptReadPort>,
    write: Arc<dyn DeptWritePort>,
    authorization_cache: AuthorizationCache,
}

impl DeptService {
    pub fn new(
        read: Arc<dyn DeptReadPort>,
        write: Arc<dyn DeptWritePort>,
        authorization_cache: AuthorizationCache,
    ) -> Self {
        Self {
            read,
            write,
            authorization_cache,
        }
    }
}

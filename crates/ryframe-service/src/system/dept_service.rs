use ryframe_db::DatabaseCluster;
use ryframe_db::DeptRepository;

use crate::AuthorizationCache;

mod commands;
mod model;
mod queries;

pub use model::{CreateDeptCommand, DeptTreeNode, DeptVo, UpdateDeptCommand};

const CACHE_TTL_SECS: u64 = 3600;
const DEPT_TREE_CACHE_NAMESPACE: &str = "dept-tree";

pub struct DeptService {
    db: DatabaseCluster,
    dept_repo: DeptRepository,
    authorization_cache: AuthorizationCache,
}

impl DeptService {
    pub fn new(db: DatabaseCluster, authorization_cache: AuthorizationCache) -> Self {
        Self {
            db,
            dept_repo: DeptRepository,
            authorization_cache,
        }
    }
}

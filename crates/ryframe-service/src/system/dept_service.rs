use ryframe_core::{LoggedRepo, RedisClient};
use ryframe_db::DatabaseCluster;
use ryframe_db::DeptRepository;

mod commands;
mod model;
mod queries;

pub use model::{CreateDeptCommand, DeptTreeNode, DeptVo, UpdateDeptCommand};

const CACHE_TTL_SECS: u64 = 3600;

fn dept_tree_cache_key(tenant_id: &str) -> String {
    format!("tenant:{tenant_id}:sys_dept:tree")
}

pub struct DeptService {
    db: DatabaseCluster,
    dept_repo: LoggedRepo<DeptRepository>,
    redis: Option<RedisClient>,
}

impl DeptService {
    pub fn new(db: DatabaseCluster, redis: Option<RedisClient>) -> Self {
        Self {
            db,
            dept_repo: LoggedRepo::new(DeptRepository),
            redis,
        }
    }

    async fn invalidate_dept_cache(&self, tenant_id: &str) {
        if let Some(redis) = &self.redis
            && let Err(error) = redis.del(dept_tree_cache_key(tenant_id)).await
        {
            tracing::warn!(tenant_id, %error, "failed to invalidate department tree cache");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::dept_tree_cache_key;

    #[test]
    fn cache_key_is_tenant_scoped() {
        let first = dept_tree_cache_key("tenant-a");
        let second = dept_tree_cache_key("tenant-b");
        assert_eq!(first, "tenant:tenant-a:sys_dept:tree");
        assert_eq!(second, "tenant:tenant-b:sys_dept:tree");
        assert_ne!(first, second);
    }
}

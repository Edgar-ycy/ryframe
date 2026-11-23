use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{
    document::{array, exact},
    domain::product_fields,
};
use crate::Result;

// 字段顺序对应产品 RestorePlan 与 RestoreDatabase 的序列化顺序。
#[derive(Serialize)]
struct OrderedPlan<'a> {
    id: &'a Value,
    backup_id: &'a Value,
    scope_id: &'a Value,
    fault_at: &'a Value,
    databases: Vec<OrderedDatabase<'a>>,
    object_endpoint: &'a Value,
    object_prefix: &'a Value,
    api_ready_url: &'a Value,
    worker_ready_url: &'a Value,
    frontend_sha: &'a Value,
}

#[derive(Serialize)]
struct OrderedDatabase<'a> {
    source_key: &'a Value,
    target_key: &'a Value,
    server_uuid: &'a Value,
    database: &'a Value,
}

pub(crate) fn product_hash(value: &Value) -> Result<String> {
    exact(value, &product_fields(), "参考协调参数")?;
    let databases = array(&value["databases"], "产品数据库")?
        .iter()
        .map(|database| {
            exact(
                database,
                &["source_key", "target_key", "server_uuid", "database"],
                "参考协调参数",
            )?;
            Ok(OrderedDatabase {
                source_key: &database["source_key"],
                target_key: &database["target_key"],
                server_uuid: &database["server_uuid"],
                database: &database["database"],
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let ordered = OrderedPlan {
        id: &value["id"],
        backup_id: &value["backup_id"],
        scope_id: &value["scope_id"],
        fault_at: &value["fault_at"],
        databases,
        object_endpoint: &value["object_endpoint"],
        object_prefix: &value["object_prefix"],
        api_ready_url: &value["api_ready_url"],
        worker_ready_url: &value["worker_ready_url"],
        frontend_sha: &value["frontend_sha"],
    };
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(&ordered)?)))
}

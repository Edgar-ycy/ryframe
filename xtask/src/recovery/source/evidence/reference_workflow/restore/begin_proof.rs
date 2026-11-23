//! 从正式恢复前像投影产品开始恢复所需的逐库逐表证明。
use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::super::proof::product_hash;
use crate::{
    Result,
    recovery::{
        clone::native::digest::plan_hash,
        inputs::{
            document::{array, digest, exact, integer, object, text},
            domain::{identifier, scope_identifier},
        },
    },
};

pub(super) fn build(
    product_plan: &Value,
    target_plan_sha256: &str,
    initialized_sha256: &str,
    expected_before: &Value,
) -> Result<Value> {
    let product_plan_hash = product_hash(product_plan)?;
    let target_plan_sha256 = digest(&Value::from(target_plan_sha256), "正式目标计划摘要")?;
    let initialized_sha256 = digest(&Value::from(initialized_sha256), "正式目标初始化摘要")?;
    let scope = text(&product_plan["scope_id"], "产品计划目标 scope")?;
    scope_identifier(scope)?;

    let planned = array(&product_plan["databases"], "产品计划数据库")?;
    let observed = object(&expected_before["databases"], "恢复前像数据库")?;
    if planned.is_empty() || planned.len() != observed.len() {
        return Err("恢复前像必须精确覆盖产品计划的目标数据库".into());
    }
    let mut source_keys = BTreeSet::new();
    let mut target_keys = BTreeSet::new();
    let mut database_names = BTreeSet::new();
    let mut databases = Vec::with_capacity(planned.len());
    for database in planned {
        let source_key = text(&database["source_key"], "产品计划源数据库键")?;
        let target_key = text(&database["target_key"], "产品计划目标数据库键")?;
        let uuid = text(&database["server_uuid"], "产品计划目标服务器 UUID")?;
        let name = text(&database["database"], "产品计划目标物理库名")?;
        for value in [source_key, target_key, uuid, name] {
            identifier(value)?;
        }
        if !source_keys.insert(source_key)
            || !target_keys.insert(target_key)
            || !database_names.insert(name)
        {
            return Err("产品计划含重复的逻辑数据库键或物理库名".into());
        }
        let image = observed
            .get(name)
            .ok_or("恢复前像缺少产品计划的目标数据库")?;
        if image["resource"]
            != json!({"kind":"database","scope_id":scope,"server_uuid":uuid,"database":name})
        {
            return Err("恢复前像数据库的 scope、服务器 UUID 或物理库名与产品计划不同".into());
        }
        databases.push(json!({"target_key":target_key,"tables":tables(image)?}));
    }
    Ok(json!({
        "format_version": 1,
        "plan_hash": product_plan_hash,
        "target_plan_sha256": target_plan_sha256,
        "initialized_sha256": initialized_sha256,
        "resource_image_sha256": plan_hash(expected_before)?,
        "databases": databases,
    }))
}

fn tables(image: &Value) -> Result<Vec<Value>> {
    let all = array(&image["all_tables"], "恢复前像完整表集合")?;
    let names = all
        .iter()
        .map(|name| text(name, "恢复前像表名"))
        .collect::<Result<BTreeSet<_>>>()?;
    let business = object(&image["tables"], "恢复前像业务表")?;
    let preserved = object(&image["preserved"], "恢复前像保留表")?;
    let mut rows_by_table = BTreeMap::new();
    for (name, observation) in business.iter().chain(preserved) {
        if rows_by_table.insert(name.as_str(), observation).is_some() {
            return Err("恢复前像业务表和保留表存在重叠".into());
        }
    }
    if names.len() != all.len() || !rows_by_table.keys().copied().eq(names) {
        return Err("恢复前像业务表和保留表必须精确分割完整表集合".into());
    }
    rows_by_table
        .into_iter()
        .map(|(table, observation)| {
            exact(observation, &["rows", "sha256"], "恢复前像表观察")?;
            let rows = integer(&observation["rows"], "恢复前像表行数")?;
            let sha256 = digest(&observation["sha256"], "恢复前像表摘要")?;
            Ok(json!({"table":table,"rows":rows,"sha256":sha256}))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET_HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const INIT_HASH: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const TABLE_HASH: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

    fn fixture() -> (Value, Value) {
        let plan = json!({
            "id": "restore", "backup_id": "backup", "scope_id": "target",
            "fault_at": "2026-11-23T00:00:00Z",
            "databases": [
                {"source_key":"source-b","target_key":"target-b","server_uuid":"server-b","database":"db_b"},
                {"source_key":"source-a","target_key":"target-a","server_uuid":"server-a","database":"db_a"},
            ],
            "object_endpoint": "http://127.0.0.1:9000", "object_prefix": "target/",
            "api_ready_url": "http://127.0.0.1:8080/ready",
            "worker_ready_url": "http://127.0.0.1:8081/ready", "frontend_sha": "d".repeat(40),
        });
        let before = json!({
            "databases": {
                "db_a": {
                    "resource":{"kind":"database","scope_id":"target","server_uuid":"server-a","database":"db_a"},
                    "tables":{"only_business":{"rows":0,"sha256":TABLE_HASH}},
                    "preserved":{}, "all_tables":["only_business"],
                },
                "db_b": {
                    "resource":{"kind":"database","scope_id":"target","server_uuid":"server-b","database":"db_b"},
                    "tables":{"z_business":{"rows":0,"sha256":TABLE_HASH}},
                    "preserved":{"a_preserved":{"rows":2,"sha256":INIT_HASH}},
                    "all_tables":["z_business","a_preserved"],
                },
            },
            "objects": {"uploads":{}},
            "redis": {"namespace":"target"},
        });
        (plan, before)
    }

    #[test]
    fn proof_keeps_plan_order_and_sorts_all_table_observations() {
        let (plan, before) = fixture();
        assert_eq!(
            build(&plan, TARGET_HASH, INIT_HASH, &before).unwrap(),
            json!({
                "format_version":1,
                "plan_hash":product_hash(&plan).unwrap(),
                "target_plan_sha256":TARGET_HASH,
                "initialized_sha256":INIT_HASH,
                "resource_image_sha256":plan_hash(&before).unwrap(),
                "databases":[
                    {"target_key":"target-b","tables":[
                        {"table":"a_preserved","rows":2,"sha256":INIT_HASH},
                        {"table":"z_business","rows":0,"sha256":TABLE_HASH},
                    ]},
                    {"target_key":"target-a","tables":[
                        {"table":"only_business","rows":0,"sha256":TABLE_HASH},
                    ]},
                ],
            })
        );
    }

    #[test]
    fn proof_rejects_missing_extra_or_drifting_database_observations() {
        let (plan, before) = fixture();
        let mut missing = before.clone();
        let removed = missing["databases"]
            .as_object_mut()
            .unwrap()
            .remove("db_a")
            .unwrap();
        assert!(build(&plan, TARGET_HASH, INIT_HASH, &missing).is_err());
        missing["databases"]["db_c"] = removed;
        assert!(build(&plan, TARGET_HASH, INIT_HASH, &missing).is_err());
        let mut extra = before.clone();
        extra["databases"]["db_c"] = extra["databases"]["db_a"].clone();
        assert!(build(&plan, TARGET_HASH, INIT_HASH, &extra).is_err());
        for key in ["scope_id", "server_uuid", "database"] {
            let mut drift = before.clone();
            drift["databases"]["db_a"]["resource"][key] = json!("unexpected");
            assert!(
                build(&plan, TARGET_HASH, INIT_HASH, &drift).is_err(),
                "{key}"
            );
        }
    }

    #[test]
    fn proof_rejects_duplicate_database_keys_or_names() {
        let (plan, before) = fixture();
        for key in ["source_key", "target_key", "database"] {
            let mut duplicate = plan.clone();
            duplicate["databases"][1][key] = duplicate["databases"][0][key].clone();
            assert!(
                build(&duplicate, TARGET_HASH, INIT_HASH, &before).is_err(),
                "{key}"
            );
        }
    }

    #[test]
    fn proof_rejects_incomplete_duplicate_or_invalid_table_observations() {
        let (plan, before) = fixture();
        for (pointer, replacement) in [
            ("/all_tables", json!(["a_preserved"])),
            (
                "/all_tables",
                json!(["a_preserved", "z_business", "z_business"]),
            ),
            (
                "/all_tables",
                json!(["a_preserved", "z_business", "unobserved"]),
            ),
            ("/tables", json!({})),
            (
                "/preserved",
                json!({"a_preserved":{"rows":2,"sha256":INIT_HASH},"z_business":{"rows":0,"sha256":TABLE_HASH}}),
            ),
            ("/tables/z_business/rows", json!(1.5)),
            ("/tables/z_business/rows", json!(-1)),
            ("/tables/z_business/sha256", json!("invalid")),
        ] {
            let mut invalid = before.clone();
            *invalid["databases"]["db_b"].pointer_mut(pointer).unwrap() = replacement;
            assert!(
                build(&plan, TARGET_HASH, INIT_HASH, &invalid).is_err(),
                "{pointer}"
            );
        }
        assert!(build(&plan, "invalid", INIT_HASH, &before).is_err());
        assert!(build(&plan, TARGET_HASH, "invalid", &before).is_err());
    }
}

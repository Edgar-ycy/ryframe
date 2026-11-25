//! 核对 Redis 归属标记和哨兵是否属于原目标初始化后像。

use serde_json::json;

use crate::{
    Result,
    recovery::clone::native::{
        copy_transfer::target_port::Target, json as document, resources::common,
    },
};

pub(super) fn require_marker_model(target: &Target) -> Result<()> {
    let scope = document::text(&target.request["target"]["scope_id"], "Redis scope")?;
    common::scope_id(scope)?;
    let owner = format!("ryframe:{{{scope}}}:.ryframe-owner");
    let sentinel = &target.request["reset"]["sentinel_value"];
    let id = document::text(&target.request["id"], "目标初始化 ID")?;
    if target.request["side"] != "seed"
        || target.request["reset"]["sentinel_key"]
            != format!("ryframe:devex-fresh:{scope}:sentinel")
        || *sentinel != format!("devex-fresh:{id}")
        || target.initialized["redis"]
            != json!({"keys":[owner],"owner":format!("ryframe-owner:v1:{scope}:redis"),"sentinel":sentinel})
    {
        return Err("目标 Redis 完整后像必须来自原成功初始化的两个标记".into());
    }
    Ok(())
}

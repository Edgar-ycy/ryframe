//! 在原登记 Redis 的认证连接中，只读核对 RustFS 移交所需标记。
use super::{Registered, Result, Value, digest, document, json, model};
use std::path::Path;

pub(in crate::recovery::clone::native::cache) fn handoff_markers(
    backend: &Path,
    request: &Value,
    runtime: &Value,
    markers: &Value,
) -> Result<Value> {
    use crate::recovery::clone::native::target_verify::redis::Client;

    document::exact(
        markers,
        &[
            "ownership_key",
            "ownership_value",
            "sentinel_key",
            "sentinel_value",
        ],
        "RustFS 移交的 Redis 标记合同",
    )?;
    let registered = Registered::open(backend, Some(request), runtime)?;
    let observed = (|| {
        registered.check_running()?;
        let port = u16::try_from(request["port"].as_u64().ok_or("Redis 端口无效")?)?;
        let password = model::password(&request["password"])?;
        let redis = &runtime["redis"];
        let mut client = Client::authenticated_windows(port, redis, password)?;
        for (key, expected) in [
            ("ownership_key", "ownership_value"),
            ("sentinel_key", "sentinel_value"),
        ] {
            let name = document::text(&markers[key], "Redis 标记键")?;
            if client.command(&["GET", name])? != markers[expected]
                || !markers[expected].is_string()
            {
                return Err("RustFS 移交前后 Redis owner 或 sentinel 与原登记不同".into());
            }
        }
        client.recheck_windows_peer()?;
        registered.check_running()?;
        Ok(
            json!({"process":runtime["process"],"run_id":redis["run_id"],
            "markers_sha256":digest::plan_hash(markers)?}),
        )
    })();
    registered.finish(observed)
}

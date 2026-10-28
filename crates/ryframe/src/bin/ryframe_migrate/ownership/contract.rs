use ryframe_sdk::RyFrameBusinessModule;
use std::error::Error;

/// 以固定非认证占位值规范化正式初始化 SQL，只计算指纹，不发布或执行该 SQL。
pub(super) fn fingerprint(
    scope: &str,
    modules: &[RyFrameBusinessModule],
) -> Result<String, Box<dyn Error>> {
    let migrations = modules
        .iter()
        .flat_map(|m| m.migrations())
        .map(|m| m.as_ref())
        .collect::<Vec<_>>();
    let hashes = ryframe_db::migration::BootstrapHashes::new(
        "$argon2id$initialization-contract-not-authentication".into(),
        "$argon2id$initialization-contract-not-authentication".into(),
    )?;
    let sql = ryframe::business::shared_control_initialization_sql(scope, &hashes, &migrations)?;
    let bindings = migrations
        .iter()
        .map(|m| {
            (
                m.scope(),
                m.module(),
                m.id(),
                m.checksum(),
                m.schema().before_fingerprint,
                m.schema().after_fingerprint,
            )
        })
        .collect::<Vec<_>>();
    Ok(super::state::digest(&serde_json::to_vec(&(sql, bindings))?))
}

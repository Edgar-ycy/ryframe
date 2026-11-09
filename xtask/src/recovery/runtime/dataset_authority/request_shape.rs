//! 各历史代次的启动请求也必须使用同一严格形状，不能只校验当前代次。
use super::{check, path};
use crate::{
    Result,
    recovery::{
        clone::native::json as strict,
        inputs::{document, domain},
    },
};
use serde_json::Value;

pub(super) fn validate(value: &Value) -> Result<()> {
    authority(&value["authority"])?;
    strict::exact(
        &value["registration"],
        &["registration", "target_plan"],
        "恢复请求登记",
    )?;
    for key in ["registration", "target_plan"] {
        document::binding_shape(&value["registration"][key], "恢复请求登记描述")?;
    }
    strict::exact(
        &value["roots"],
        &["backend_product", "backend_execution", "frontend"],
        "恢复请求来源根",
    )?;
    strict::exact(
        &value["paths"],
        &["bindings", "backend_build", "frontend_build"],
        "恢复请求文件",
    )?;
    strict::exact(
        &value["digests"],
        &["bindings", "backend_build", "frontend_build"],
        "恢复请求摘要",
    )?;
    for field in ["roots", "paths"] {
        for item in strict::object(&value[field], "恢复请求路径")?.values() {
            path(item)?;
        }
    }
    for item in strict::object(&value["digests"], "恢复请求摘要")?.values() {
        document::digest(item, "恢复请求摘要")?;
    }
    strict::exact(&value["artifacts"], &["api", "worker"], "恢复请求产物")?;
    for role in ["api", "worker"] {
        let item = &value["artifacts"][role];
        strict::exact(
            item,
            &["executable", "command", "bytes", "sha256"],
            "恢复请求角色产物",
        )?;
        path(&item["executable"])?;
        strict::array(&item["command"], "恢复角色构建命令")?;
        check(
            item["bytes"].as_u64().is_some_and(|bytes| bytes > 0),
            "恢复角色产物必须非空",
        )?;
        document::digest(&item["sha256"], "恢复角色产物摘要")?;
    }
    check(
        (value["authority"]["backend_product_sha"] == value["authority"]["backend_execution_sha"])
            == (value["roots"]["backend_product"] == value["roots"]["backend_execution"]),
        "恢复产品和执行路径不符合适配关系",
    )?;
    document::binding_shape(&value["environment"]["document"], "恢复环境文件")?;
    for role in ["api", "worker"] {
        configuration(&value["environment"]["configurations"][role])?;
    }
    Ok(())
}

fn configuration(value: &Value) -> Result<()> {
    strict::exact(
        value,
        &["directory", "files", "environment_sha256"],
        "恢复请求配置快照",
    )?;
    let directory = path(&value["directory"])?;
    document::digest(&value["environment_sha256"], "恢复请求环境摘要")?;
    let files = strict::object(&value["files"], "恢复请求配置文件")?;
    check(
        files.contains_key("app.test.toml"),
        "恢复请求缺少所选环境文件",
    )?;
    for (name, binding) in files {
        check(
            !name.contains(['/', '\\']) && name.ends_with(".toml"),
            "恢复请求配置文件名无效",
        )?;
        strict::exact(binding, &["bytes", "sha256"], "恢复请求配置文件描述")?;
        document::binding_shape(
            &serde_json::json!({"path":directory.join(name),"bytes":binding["bytes"],"sha256":binding["sha256"]}),
            "恢复请求配置文件",
        )?;
    }
    Ok(())
}

pub(in crate::recovery::runtime) fn authority(value: &Value) -> Result<()> {
    strict::exact(
        value,
        &[
            "format_version",
            "kind",
            "restore_id",
            "backup_id",
            "plan_hash",
            "scope_id",
            "data_verified_at",
            "backup_source_sha",
            "backend_product_sha",
            "backend_execution_sha",
            "backend_adapter_contract",
            "frontend_sha",
            "api_endpoint",
            "worker_endpoint",
            "frontend_endpoint",
        ],
        "恢复请求权威",
    )?;
    check(
        value["format_version"] == 2 && value["kind"] == "restore-runtime-authority",
        "恢复请求权威类型无效",
    )?;
    for key in ["restore_id", "backup_id"] {
        domain::identifier(strict::text(&value[key], "恢复请求 ID")?)?;
    }
    domain::scope_identifier(strict::text(&value["scope_id"], "恢复请求 scope")?)?;
    domain::timestamp(&value["data_verified_at"], "恢复请求校验时间")?;
    document::digest(&value["plan_hash"], "恢复请求计划摘要")?;
    for key in [
        "backup_source_sha",
        "backend_product_sha",
        "backend_execution_sha",
        "frontend_sha",
    ] {
        domain::hash_40(&value[key], "恢复请求源码")?;
    }
    let same = value["backend_product_sha"] == value["backend_execution_sha"];
    check(
        if same {
            value["backend_adapter_contract"].is_null()
        } else {
            value["backend_adapter_contract"] == "legacy-stable-readiness-b0-v1"
        },
        "恢复请求适配关系无效",
    )?;
    super::schema::endpoints(
        &serde_json::json!({"api":value["api_endpoint"],"worker":value["worker_endpoint"],"frontend":value["frontend_endpoint"]}),
    )
}

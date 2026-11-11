//! 精确冻结 B0 的启动资格、角色目录和三个历史 MySQL 变量名。
use serde_json::Value;

pub(in crate::recovery::runtime) const PASSWORD_VARIABLES: [&str; 3] = [
    "RYFRAME_B0_MYSQL_PASSWORD_1",
    "RYFRAME_B0_MYSQL_PASSWORD_2",
    "RYFRAME_B0_MYSQL_PASSWORD_3",
];

pub(in crate::recovery::runtime) fn matches(authority: &Value) -> bool {
    authority["frontend_sha"] == crate::recovery::frontend_build::b0::HEAD
        && authority["backend_adapter_contract"] == "legacy-stable-readiness-b0-v1"
        && authority["backend_product_sha"] == "815c5eafb09d4b493319d255fb7ab88ddd02c8b6"
        && authority["backend_execution_sha"] == "c05114bcdf5c369cd74087db6317ce3c8f89bee8"
}

pub(in crate::recovery::runtime) fn config_directory(role: &str) -> Option<&'static str> {
    match role {
        "api" => Some("b0-config-api"),
        "worker" => Some("b0-config-worker"),
        _ => None,
    }
}

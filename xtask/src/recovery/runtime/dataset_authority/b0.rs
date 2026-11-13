//! 精确冻结 B0 的启动资格、角色目录和三个历史 MySQL 变量名。

pub(in crate::recovery::runtime) const PASSWORD_VARIABLES: [&str; 3] = [
    "RYFRAME_B0_MYSQL_PASSWORD_1",
    "RYFRAME_B0_MYSQL_PASSWORD_2",
    "RYFRAME_B0_MYSQL_PASSWORD_3",
];

pub(in crate::recovery::runtime) fn config_directory(role: &str) -> Option<&'static str> {
    match role {
        "api" => Some("b0-config-api"),
        "worker" => Some("b0-config-worker"),
        _ => None,
    }
}

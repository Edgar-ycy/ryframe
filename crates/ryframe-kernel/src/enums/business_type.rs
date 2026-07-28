use serde::{Deserialize, Serialize};

/// 业务操作类型，用于操作日志记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum BusinessType {
    /// 其它操作。
    Other,
    /// 查询操作。
    Query,
    /// 新增操作。
    Insert,
    /// 修改操作。
    Update,
    /// 删除操作。
    Delete,
    /// 导出操作。
    Export,
    /// 导入操作。
    Import,
    /// 授权操作。
    Grant,
    /// 强退操作。
    ForceLogout,
    /// 清空数据操作。
    Clean,
}

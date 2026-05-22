use serde::{Deserialize, Serialize};

/// 业务操作类型（用于操作日志记录）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum BusinessType {
    /// 其它
    Other,
    /// 查询
    Query,
    /// 新增
    Insert,
    /// 修改
    Update,
    /// 删除
    Delete,
    /// 导出
    Export,
    /// 导入
    Import,
    /// 授权
    Grant,
    /// 强退
    ForceLogout,
    /// 清空数据
    Clean,
}
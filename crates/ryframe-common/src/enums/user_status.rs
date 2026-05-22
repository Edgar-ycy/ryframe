use serde::{Deserialize, Serialize};

/// 用户状态
///
/// - Normal: 正常可登录
/// - Disabled: 管理员手动停用
/// - Locked: 密码错误次数过多自动锁定
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UserStatus {
    /// 正常
    Normal,
    /// 停用
    Disabled,
    /// 锁定
    Locked,
}

impl UserStatus {
    /// 是否允许登录
    pub fn can_login(&self) -> bool {
        matches!(self, UserStatus::Normal)
    }
}
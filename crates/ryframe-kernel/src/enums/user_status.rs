use serde::{Deserialize, Serialize};

/// 用户状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UserStatus {
    /// 正常。
    Normal,
    /// 停用。
    Disabled,
    /// 锁定。
    Locked,
}

impl UserStatus {
    /// 判断当前状态是否允许登录。
    pub fn can_login(&self) -> bool {
        matches!(self, Self::Normal)
    }
}

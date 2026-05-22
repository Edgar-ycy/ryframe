use serde::Serialize;

/// 在线用户信息
#[derive(Debug, Clone, Serialize)]
pub struct OnlineUserVo {
    /// 令牌 ID
    pub token_id: String,
    /// 用户名
    pub username: String,
    /// 部门名称
    pub dept_name: Option<String>,
    /// 登录 IP
    pub ipaddr: String,
    /// 登录地点
    pub login_location: Option<String>,
    /// 浏览器
    pub browser: Option<String>,
    /// 操作系统
    pub os: Option<String>,
    /// 登录时间
    pub login_time: String,
    /// 最后访问时间
    pub last_access_time: String,
}

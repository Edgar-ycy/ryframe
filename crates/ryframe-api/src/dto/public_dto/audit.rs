use serde::Serialize;
use utoipa::ToSchema;

use ryframe_service::system::{
    LoginInfoVo as ServiceLoginInfoVo, OnlineUserVo as ServiceOnlineUserVo,
    OperLogVo as ServiceOperLogVo,
};

/// 登录日志响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct LoginInfoVo {
    pub id: String,
    pub user_name: String,
    pub ipaddr: String,
    pub login_location: Option<String>,
    pub browser: Option<String>,
    pub os: Option<String>,
    pub status: String,
    pub msg: Option<String>,
    pub login_time: String,
}

impl From<ServiceLoginInfoVo> for LoginInfoVo {
    fn from(value: ServiceLoginInfoVo) -> Self {
        let ServiceLoginInfoVo {
            id,
            user_name,
            ipaddr,
            login_location,
            browser,
            os,
            status,
            msg,
            login_time,
        } = value;
        Self {
            id,
            user_name,
            ipaddr,
            login_location,
            browser,
            os,
            status,
            msg,
            login_time,
        }
    }
}

/// 在线用户响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct OnlineUserVo {
    pub sid: String,
    pub username: String,
    pub dept_name: Option<String>,
    pub ipaddr: String,
    pub login_location: Option<String>,
    pub browser: Option<String>,
    pub os: Option<String>,
    pub login_time: String,
    pub last_access_time: String,
}

impl From<ServiceOnlineUserVo> for OnlineUserVo {
    fn from(value: ServiceOnlineUserVo) -> Self {
        let ServiceOnlineUserVo {
            sid,
            username,
            dept_name,
            ipaddr,
            login_location,
            browser,
            os,
            login_time,
            last_access_time,
        } = value;
        Self {
            sid,
            username,
            dept_name,
            ipaddr,
            login_location,
            browser,
            os,
            login_time,
            last_access_time,
        }
    }
}

/// 操作日志响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct OperLogVo {
    pub id: String,
    pub title: String,
    pub business_type: String,
    pub method: String,
    pub request_method: String,
    pub oper_name: String,
    pub oper_url: String,
    pub oper_ip: String,
    pub oper_location: Option<String>,
    pub oper_param: Option<String>,
    pub json_result: Option<String>,
    pub status: String,
    pub error_msg: Option<String>,
    pub cost_time: i64,
    pub oper_time: String,
}

impl From<ServiceOperLogVo> for OperLogVo {
    fn from(value: ServiceOperLogVo) -> Self {
        let ServiceOperLogVo {
            id,
            title,
            business_type,
            method,
            request_method,
            oper_name,
            oper_url,
            oper_ip,
            oper_location,
            oper_param,
            json_result,
            status,
            error_msg,
            cost_time,
            oper_time,
        } = value;
        Self {
            id,
            title,
            business_type,
            method,
            request_method,
            oper_name,
            oper_url,
            oper_ip,
            oper_location,
            oper_param,
            json_result,
            status,
            error_msg,
            cost_time,
            oper_time,
        }
    }
}

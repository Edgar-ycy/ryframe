use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use ryframe_http::HttpResult;
use ryframe_i18n::LocalizedText;
use ryframe_kernel::AppError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

/// 发布消息时的受众选择器。
#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MessageAudienceDto {
    /// tenant、role 或 user。
    pub kind: String,
    /// tenant 受众必须省略或传入 "0"；角色和用户 ID 以字符串避免精度丢失。
    pub target_id: Option<String>,
}

/// 创建消息请求。
#[derive(Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PublishMessageDto {
    /// 省略时投递给当前租户；指定其他租户时需要平台级发布权限。
    #[validate(length(
        min = 1,
        max = 64,
        message = "目标租户标识长度必须在 1 到 64 个字符之间"
    ))]
    pub tenant_id: Option<String>,
    #[validate(length(min = 1, max = 64, message = "消息主题长度必须在 1 到 64 个字符之间"))]
    pub topic: String,
    #[validate(length(min = 1, max = 200, message = "消息标题长度必须在 1 到 200 个字符之间"))]
    pub title: Option<String>,
    #[validate(length(
        min = 1,
        max = 10_000,
        message = "消息正文长度必须在 1 到 10000 个字符之间"
    ))]
    pub content: Option<String>,
    #[validate(length(
        min = 1,
        max = 128,
        message = "消息标题本地化键长度必须在 1 到 128 个字符之间"
    ))]
    pub title_key: Option<String>,
    #[validate(length(
        min = 1,
        max = 128,
        message = "消息正文本地化键长度必须在 1 到 128 个字符之间"
    ))]
    pub body_key: Option<String>,
    #[serde(default)]
    #[validate(length(max = 50, message = "消息本地化参数不能超过 50 个"))]
    pub args: BTreeMap<String, String>,
    pub severity: String,
    pub payload: Option<Value>,
    #[validate(length(max = 64, message = "来源类型不能超过 64 个字符"))]
    pub source_type: Option<String>,
    #[validate(length(max = 128, message = "来源标识不能超过 128 个字符"))]
    pub source_id: Option<String>,
    #[validate(length(min = 1, max = 500, message = "消息受众数量必须在 1 到 500 之间"))]
    pub audiences: Vec<MessageAudienceDto>,
    /// 可选的提前过期时间，最长仍受服务端 90 天上限约束。
    pub expires_at: Option<DateTime<Utc>>,
}

impl PublishMessageDto {
    /// 将 HTTP 输入规范化为服务层可校验的本地化文本。
    pub fn localized_content(&self) -> HttpResult<(LocalizedText, LocalizedText)> {
        match (&self.title, &self.content, &self.title_key, &self.body_key) {
            (Some(title), Some(content), None, None) if self.args.is_empty() => Ok((
                LocalizedText::Literal {
                    value: title.clone(),
                },
                LocalizedText::Literal {
                    value: content.clone(),
                },
            )),
            (None, None, Some(title_key), Some(body_key)) => {
                validate_localized_args(&self.args)?;
                Ok((
                    LocalizedText::Key {
                        key: title_key.clone(),
                        args: self.args.clone(),
                    },
                    LocalizedText::Key {
                        key: body_key.clone(),
                        args: self.args.clone(),
                    },
                ))
            }
            _ => Err(AppError::Validation(
                "消息标题和正文必须同时提供纯文本，或同时提供本地化键".into(),
            )
            .into()),
        }
    }
}

fn validate_localized_args(args: &BTreeMap<String, String>) -> HttpResult<()> {
    if args
        .iter()
        .any(|(key, value)| key.trim().is_empty() || key.len() > 64 || value.len() > 512)
    {
        return Err(AppError::Validation(
            "消息本地化参数的键不能为空且最长 64 个字符，值最长 512 个字符".into(),
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::PublishMessageDto;
    use validator::Validate;

    #[test]
    fn accepts_literal_message_payload() {
        let dto: PublishMessageDto = serde_json::from_value(serde_json::json!({
            "topic": "system",
            "title": "标题",
            "content": "正文",
            "severity": "info",
            "audiences": [{ "kind": "tenant" }]
        }))
        .expect("纯文本消息请求");

        dto.validate().expect("字段校验");
        assert!(dto.localized_content().is_ok());
    }

    #[test]
    fn accepts_keyed_message_payload_with_named_arguments() {
        let dto: PublishMessageDto = serde_json::from_value(serde_json::json!({
            "topic": "system",
            "title_key": "user.welcome",
            "body_key": "user.welcome",
            "args": { "name": "Ada" },
            "severity": "info",
            "audiences": [{ "kind": "tenant" }]
        }))
        .expect("本地化消息请求");

        dto.validate().expect("字段校验");
        assert!(dto.localized_content().is_ok());
    }

    #[test]
    fn rejects_mixed_literal_and_keyed_message_payload() {
        let dto: PublishMessageDto = serde_json::from_value(serde_json::json!({
            "topic": "system",
            "title": "标题",
            "body_key": "user.welcome",
            "severity": "info",
            "audiences": [{ "kind": "tenant" }]
        }))
        .expect("混合消息请求");

        assert!(dto.localized_content().is_err());
    }
}

/// 收件箱游标查询参数。
#[derive(Debug, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
pub struct MessageInboxQuery {
    pub cursor: Option<String>,
    pub limit: Option<u64>,
    #[serde(default)]
    pub unread_only: bool,
}

/// 批量确认消息请求。
#[derive(Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AcknowledgeMessagesDto {
    #[validate(length(min = 1, max = 100, message = "一次最多确认 100 条消息"))]
    pub ids: Vec<String>,
}

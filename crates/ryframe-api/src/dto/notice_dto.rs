use serde::Deserialize;
use utoipa::ToSchema;
use validator::ValidationError;

pub const NOTICE_MARKDOWN_MIN_UTF8_BYTES: usize = 1;
pub const NOTICE_MARKDOWN_MAX_UTF8_BYTES: usize = 60_000;

fn validate_notice_markdown(value: &str) -> Result<(), ValidationError> {
    let byte_length = value.len();
    if (NOTICE_MARKDOWN_MIN_UTF8_BYTES..=NOTICE_MARKDOWN_MAX_UTF8_BYTES).contains(&byte_length) {
        return Ok(());
    }

    let mut error = ValidationError::new("notice_markdown_utf8_bytes");
    error.message = Some(
        format!(
            "Markdown 内容必须为 {NOTICE_MARKDOWN_MIN_UTF8_BYTES} 到 \
             {NOTICE_MARKDOWN_MAX_UTF8_BYTES} 个 UTF-8 字节，当前为 {byte_length} 字节"
        )
        .into(),
    );
    Err(error)
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateNoticeDto {
    #[validate(length(min = 1, max = 200, message = "标题长度必须在 1 到 200 个字符之间"))]
    pub title: String,
    #[validate(custom(function = "validate_notice_markdown"))]
    #[schema(min_length = 1, max_length = 60_000)]
    /// 公告 Markdown 原文，长度为 1 到 60,000 个 UTF-8 字节。
    pub content_markdown: String,
    pub notice_type: Option<String>,
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateNoticeDto {
    #[validate(length(min = 1, max = 200, message = "标题长度必须在 1 到 200 个字符之间"))]
    pub title: String,
    #[validate(custom(function = "validate_notice_markdown"))]
    #[schema(min_length = 1, max_length = 60_000)]
    /// 公告 Markdown 原文，长度为 1 到 60,000 个 UTF-8 字节。
    pub content_markdown: String,
    pub notice_type: Option<String>,
    pub status: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use validator::Validate;

    fn create_dto(content_markdown: String) -> CreateNoticeDto {
        CreateNoticeDto {
            title: "公告".into(),
            content_markdown,
            notice_type: Some("notice".into()),
        }
    }

    #[test]
    fn markdown_length_is_measured_in_utf8_bytes() {
        assert!(create_dto("a".into()).validate().is_ok());
        assert!(create_dto("a".repeat(60_000)).validate().is_ok());
        assert!(create_dto("中".repeat(20_000)).validate().is_ok());

        assert!(create_dto(String::new()).validate().is_err());
        assert!(create_dto("a".repeat(60_001)).validate().is_err());
        assert!(create_dto("中".repeat(20_001)).validate().is_err());
    }

    #[test]
    fn legacy_content_field_is_rejected() {
        let error = serde_json::from_value::<CreateNoticeDto>(serde_json::json!({
            "title": "公告",
            "content": "旧字段",
            "notice_type": "notice"
        }))
        .unwrap_err();

        assert!(error.to_string().contains("unknown field `content`"));
    }
}

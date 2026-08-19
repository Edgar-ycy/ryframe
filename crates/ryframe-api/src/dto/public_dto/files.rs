use crate::http::api_path;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Serialize;
use utoipa::ToSchema;

use ryframe_application::system::UploadResponse as ServiceUploadResponse;

/// 文件上传响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct UploadResponse {
    pub file_id: String,
    pub file_name: String,
    pub file_path: String,
    pub file_url: String,
}

/// 构造只能通过认证 API 下载的私有文件地址。
pub(crate) fn private_file_url(bucket: &str, path: &str) -> String {
    format!(
        "{}?bucket={}&path={}",
        api_path("common/file/download"),
        utf8_percent_encode(bucket, NON_ALPHANUMERIC),
        utf8_percent_encode(path, NON_ALPHANUMERIC),
    )
}

impl From<ServiceUploadResponse> for UploadResponse {
    fn from(value: ServiceUploadResponse) -> Self {
        let ServiceUploadResponse {
            file_id,
            bucket,
            file_name,
            file_path,
        } = value;
        Self {
            file_id,
            file_url: private_file_url(&bucket, &file_path),
            file_name,
            file_path,
        }
    }
}

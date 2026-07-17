use utoipa::ToSchema;

#[derive(Debug, ToSchema)]
pub struct FileUploadForm {
    #[schema(content_media_type = "application/octet-stream")]
    pub file: Vec<u8>,
}

use super::{extensions, operations};

/// 向 Utoipa 生成的文档合并资源切片，并补充 RyFrame 的运行时契约。
pub(super) struct ApiDocModifier;

impl utoipa::Modify for ApiDocModifier {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        extensions::merge_generated_document(openapi);
        extensions::configure_security_schemes(openapi);
        extensions::add_contract_extensions(openapi);
        operations::finalize_operations(openapi);
    }
}

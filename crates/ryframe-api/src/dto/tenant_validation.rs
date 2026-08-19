use validator::ValidationError;

pub(super) fn validate_tenant_identifier(value: &str) -> Result<(), ValidationError> {
    ryframe_kernel::TenantId::parse(value)
        .map(|_| ())
        .map_err(|_| {
            let mut error = ValidationError::new("tenant_identifier");
            error.message = Some(
                "tenant ID must contain only ASCII letters, digits, hyphens, or underscores".into(),
            );
            error
        })
}

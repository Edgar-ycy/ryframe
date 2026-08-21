use ryframe_application::ports::jobs::ExecutionTenantScope;

pub(crate) fn database_scope(
    scope: &ExecutionTenantScope,
) -> crate::repositories::ExecutionTenantFilter {
    scope.tenant_id().map_or_else(
        crate::repositories::ExecutionTenantFilter::all,
        crate::repositories::ExecutionTenantFilter::tenant_and_platform,
    )
}

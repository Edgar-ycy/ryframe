use ryframe_application::ExecutionTenantScope;

pub(crate) fn database_scope(scope: &ExecutionTenantScope) -> crate::ExecutionTenantScope {
    scope.tenant_id().map_or_else(
        crate::ExecutionTenantScope::all,
        crate::ExecutionTenantScope::tenant_and_platform,
    )
}

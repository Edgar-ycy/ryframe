use crate::ExecutionTenantScope;

pub(crate) fn database_scope(scope: &ExecutionTenantScope) -> ryframe_db::ExecutionTenantScope {
    scope.tenant_id().map_or_else(
        ryframe_db::ExecutionTenantScope::all,
        ryframe_db::ExecutionTenantScope::tenant_and_platform,
    )
}

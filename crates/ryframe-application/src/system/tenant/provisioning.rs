use std::collections::HashSet;

use ryframe_kernel::AppResult;

use crate::ports::tenants::{
    ProvisionTenantRecord, TenantPermissionTemplate, TenantProvisioningTemplate, TenantTransaction,
};

pub(super) async fn provision_new_tenant_in_transaction(
    transaction: &dyn TenantTransaction,
    record: &ProvisionTenantRecord,
) -> AppResult<()> {
    let template = transaction.load_provisioning_template().await?;
    let template = filter_provisioning_template(template, record);
    let identity = transaction.initialize_tenant_identity(record).await?;
    transaction
        .copy_tenant_authorization(&identity, template.authorization)
        .await?;
    transaction
        .copy_tenant_base_catalogs(&identity, template.base_catalogs)
        .await
}

fn filter_provisioning_template(
    mut template: TenantProvisioningTemplate,
    record: &ProvisionTenantRecord,
) -> TenantProvisioningTemplate {
    let managed_routes = string_set(&record.managed_capability_route_keys);
    let enabled_routes = string_set(&record.enabled_capability_route_keys);
    template.authorization.menus.retain(|menu| {
        menu.route_key.as_deref().is_none_or(|route_key| {
            !super::super::platform_boundary::is_platform_route(route_key)
                && (!managed_routes.contains(route_key) || enabled_routes.contains(route_key))
        })
    });

    filter_permissions(&mut template.authorization.permissions, record);
    let active_dictionary_types = template
        .base_catalogs
        .dictionary_types
        .iter()
        .map(|dictionary| dictionary.code.as_str())
        .collect::<HashSet<_>>();
    template
        .base_catalogs
        .dictionary_data
        .retain(|data| active_dictionary_types.contains(data.type_code.as_str()));
    template
}

fn filter_permissions(
    permissions: &mut Vec<TenantPermissionTemplate>,
    record: &ProvisionTenantRecord,
) {
    let managed = string_set(&record.managed_capability_permission_codes);
    let enabled = string_set(&record.enabled_capability_permission_codes);
    let defaults = string_set(&record.default_admin_permission_codes);
    let mut retained = permissions
        .iter()
        .filter(|permission| permission_allowed(&permission.code))
        .filter(|permission| {
            !managed.contains(permission.code.as_str())
                || enabled.contains(permission.code.as_str())
        })
        .map(|permission| permission.source_id)
        .collect::<HashSet<_>>();
    close_permission_parents(permissions, &mut retained);
    permissions.retain(|permission| {
        permission_allowed(&permission.code) && retained.contains(&permission.source_id)
    });
    for permission in permissions {
        let managed_permission = managed.contains(permission.code.as_str());
        permission.assign_admin = if managed_permission {
            defaults.contains(permission.code.as_str())
        } else {
            assign_standard_admin_permission(&permission.code)
        };
        permission.assign_user = permission.assign_admin
            && !managed_permission
            && [":query", ":list", ":view"]
                .iter()
                .any(|suffix| permission.code.ends_with(suffix));
    }
}

fn close_permission_parents(permissions: &[TenantPermissionTemplate], retained: &mut HashSet<i64>) {
    loop {
        let previous_len = retained.len();
        let parent_ids = permissions
            .iter()
            .filter(|permission| retained.contains(&permission.source_id))
            .filter_map(|permission| permission.parent_source_id)
            .collect::<Vec<_>>();
        retained.extend(parent_ids);
        if retained.len() == previous_len {
            return;
        }
    }
}

fn permission_allowed(code: &str) -> bool {
    code != "*:*:*" && !super::super::platform_boundary::is_platform_permission(code)
}

fn assign_standard_admin_permission(code: &str) -> bool {
    !code.starts_with("monitor:job:")
        && !code.starts_with("monitor:schedule:")
        && code != "monitor:overview:list"
        && !code.starts_with("system:user-import:")
        && code != "system:authorization-diagnostic:list"
}

fn string_set(values: &[String]) -> HashSet<&str> {
    values.iter().map(String::as_str).collect()
}

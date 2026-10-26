use super::super::{ResourceIr, StorageKind};
use super::queries::*;
use super::write::insert_body;

pub(crate) fn repository(resource: &ResourceIr, resources: &[&ResourceIr], header: &str) -> String {
    let scope_parameter = super::super::owner_scope_parameter(resource);
    let pascal = &resource.pascal_name;
    let filter_lifetime = super::super::filter_lifetime(resource, "'_");
    let filter_argument = if super::super::uses_filters(resource) {
        "filter"
    } else {
        "_filter"
    };
    let column_trait = if !resource.access.system_tenant_only
        || resource.soft_delete.is_some()
        || super::super::uses_filters(resource)
        || super::super::application::unique_business_indexes(resource)
            .next()
            .is_some()
        || resource.relations.iter().any(|relation| {
            resources.iter().any(|target| {
                target.name == relation.target_resource && target.soft_delete.is_some()
            })
        }) {
        "ColumnTrait, "
    } else {
        ""
    };
    let name = &resource.name;
    let persistence = persistence_parts(resource);
    let base_select = base_select(resource);
    let id_query = scoped_id_query(resource, "id", "build_token");
    let (detail_type, detail_import, mut detail_read, relation_mappers) =
        relation_detail_parts(resource, resources, &id_query);
    let filters = filter_statements(resource);
    let order = order_statements(resource);
    let to_record = mapping(resource, "model");
    let to_entity = mapping(resource, "record");
    let delete = delete_body(resource);
    let commit = commit_body(resource.storage);
    let unique_conflicts = unique_conflict_mappings(resource);
    let generic_conflict = format!("{}已存在", resource.labels.zh_cn);
    let tenant_mismatch = format!("{}事务租户不匹配", resource.labels.zh_cn);
    let pagination = match resource.storage {
        StorageKind::ControlRow => "crate::pagination::paginate",
        StorageKind::TenantData => "ryframe_db::pagination::paginate",
    };
    let model_trait = if resource.soft_delete.is_none() {
        "ModelTrait, "
    } else {
        ""
    };
    let set_import = if resource.soft_delete.is_some() {
        "ActiveValue::Set, "
    } else {
        ""
    };
    let transaction_trait = if resource.storage == StorageKind::ControlRow {
        "TransactionTrait, "
    } else {
        ""
    };
    let query_filter_trait = if column_trait.is_empty()
        && resource.id_generation != crate::IdGeneration::AutoIncrement
    {
        ""
    } else {
        "QueryFilter, "
    };
    let control_transaction_methods = control_transaction_methods(resource);
    let scoped_tenant_owner =
        resource.access.owner_field.is_some() && resource.storage == StorageKind::TenantData;
    let (data_scope_lock_method, write_scope) = projected_write_scope(resource);
    if scoped_tenant_owner {
        detail_read = detail_read.replace("&database", "database");
    }
    let write_id_query = if scoped_tenant_owner {
        scoped_id_query(resource, "id", "&build_token")
    } else {
        id_query
    };
    let pagination_db = if scoped_tenant_owner {
        "database"
    } else {
        "&database"
    };
    format!(
        include_str!("repository.rs.tpl"),
        declaration = persistence.declaration,
        read_connection = persistence.read_connection,
        begin_transaction = persistence.begin_transaction,
        read_scope_begin_by_id = persistence.read_scope_begin_by_id,
        read_scope_begin_page = persistence.read_scope_begin_page,
        read_wrap_open = persistence.read_wrap_open,
        read_wrap_close = persistence.read_wrap_close,
        pagination_db = pagination_db,
        pagination = pagination,
        generic_conflict = generic_conflict,
        header = header,
        id_type = super::super::id_type(resource),
        scope_parameter = scope_parameter,
        name = name,
        pascal = pascal,
        filter_lifetime = filter_lifetime,
        filter_argument = filter_argument,
        column_trait = column_trait,
        query_filter_trait = query_filter_trait,
        detail_import = detail_import,
        model_trait = model_trait,
        set_import = set_import,
        transaction_trait = transaction_trait,
        tenant_mismatch = tenant_mismatch,
        detail_type = detail_type,
        detail_read = detail_read,
        base_select = base_select,
        filters = filters,
        order = order,
        control_transaction_methods = control_transaction_methods,
        data_scope_lock_method = data_scope_lock_method,
        write_scope = write_scope,
        write_id_query = write_id_query,
        unique_conflicts = unique_conflicts,
        delete = delete,
        commit = commit,
        insert_type = super::super::insert_type(resource),
        insert_import = super::super::insert_import(resource),
        update_body = super::write::update_body(resource),
        to_record = to_record,
        to_entity = to_entity,
        insert_body = insert_body(resource, &unique_conflicts, &generic_conflict),
        record_tenant = if resource.access.system_tenant_only {
            "\"system\""
        } else {
            "&record.tenant_id"
        },
        relation_mappers = relation_mappers,
    ) + "\n"
}

fn projected_write_scope(resource: &ResourceIr) -> (String, String) {
    if resource.access.owner_field.is_none() || resource.storage != StorageKind::TenantData {
        return (String::new(), String::new());
    }
    let data_scope_module = if resource.module == "business" {
        "ryframe_sdk::persistence::data_scope"
    } else {
        "ryframe_db::data_scope"
    };
    (
        format!(
            "    async fn lock_data_scope(\n        &self,\n        data_scope: &ryframe_kernel::DataScopeContext,\n    ) -> AppResult<()> {{\n        {data_scope_module}::lock_projected_build(\n            &self.transaction,\n            &self.tenant_id,\n            data_scope.data_scope_version,\n        )\n        .await\n        .map(|_| ())\n    }}\n\n"
        ),
        format!(
            "        let build_token = {data_scope_module}::lock_projected_build(\n            &self.transaction,\n            tenant_id,\n            data_scope.data_scope_version,\n        )\n        .await?;\n"
        ),
    )
}

fn relation_detail_parts(
    resource: &ResourceIr,
    resources: &[&ResourceIr],
    id_query: &str,
) -> (String, String, String, String) {
    let pascal = &resource.pascal_name;
    if resource.relations.is_empty() {
        return (
            format!("{pascal}Record"),
            String::new(),
            format!(
                "        Ok({id_query}\n            .one(&database)\n            .await\n            .db()?\n            .map(to_record))"
            ),
            String::new(),
        );
    }

    let mut reads = String::new();
    let mut fields = String::new();
    let mut mappers = String::new();
    for relation in &resource.relations {
        let target = resources
            .iter()
            .copied()
            .find(|candidate| candidate.name == relation.target_resource)
            .expect("关系目标已在生成入口校验");
        let local = resource
            .fields
            .iter()
            .find(|field| field.name == relation.local_field)
            .expect("关系字段已在 IR 校验");
        let query = relation_query(target, "relation_id");
        let mapper = format!("to_{}_record", relation.name);
        if local.nullable {
            reads.push_str(&format!(
                "        let {name} = match record.{local} {{\n            Some(relation_id) => {query}\n                .one(&database)\n                .await\n                .db()?\n                .map({mapper}),\n            None => None,\n        }};\n",
                name = relation.name,
                local = relation.local_field,
            ));
        } else {
            reads.push_str(&format!(
                "        let relation_id = record.{local};\n        let {name} = {query}\n            .one(&database)\n            .await\n            .db()?\n            .map({mapper});\n",
                name = relation.name,
                local = relation.local_field,
            ));
        }
        fields.push_str(&format!("            {},\n", relation.name));
        let mapping = mapping(target, "model");
        mappers.push_str(&format!(
            "fn {mapper}(model: crate::generated::{target_name}::entity::Model) -> ryframe_application::generated::{target_name}::{target_pascal}Record {{\n    ryframe_application::generated::{target_name}::{target_pascal}Record {{\n{mapping}\n    }}\n}}\n\n",
            target_name = target.name,
            target_pascal = target.pascal_name,
        ));
    }
    let read = format!(
        "        let Some(record) = {id_query}\n            .one(&database)\n            .await\n            .db()?\n            .map(to_record)\n        else {{\n            return Ok(None);\n        }};\n{reads}        Ok(Some({pascal}Detail {{\n            record,\n{fields}        }}))"
    );
    (
        format!("{pascal}Detail"),
        format!(", {pascal}Detail"),
        read,
        mappers.trim_end().to_owned(),
    )
}

fn relation_query(target: &ResourceIr, id: &str) -> String {
    let mut query = match target.storage {
        StorageKind::ControlRow => format!(
            "crate::generated::{name}::entity::Entity::find_by_id({id})",
            name = target.name
        ),
        StorageKind::TenantData => format!(
            "crate::generated::{name}::entity::Entity::find_by_id((tenant_id.to_owned(), {id}))",
            name = target.name
        ),
    };
    if !target.access.system_tenant_only {
        query.push_str(&format!(
            "\n                .filter(crate::generated::{name}::entity::Column::TenantId.eq(tenant_id))",
            name = target.name
        ));
    }
    if let Some(soft_delete) = &target.soft_delete {
        query.push_str(&format!(
            "\n                .filter(crate::generated::{name}::entity::Column::{column}.eq({active}))",
            name = target.name,
            column = super::super::column_variant(&soft_delete.field),
            active = field_literal(target, &soft_delete.field, &soft_delete.active),
        ));
    }
    query
}

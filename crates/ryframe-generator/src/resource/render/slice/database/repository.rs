use super::super::{ResourceIr, StorageKind};
use super::queries::*;

pub(crate) fn repository(resource: &ResourceIr, resources: &[&ResourceIr], header: &str) -> String {
    let pascal = &resource.pascal_name;
    let name = &resource.name;
    let persistence = persistence_parts(resource);
    let base_select = base_select(resource);
    let id_query = id_query(resource, "id");
    let (detail_type, detail_import, detail_read, relation_mappers) =
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
    let tenant_error = if resource.storage == StorageKind::TenantData {
        tenant_error_mapper()
    } else {
        String::new()
    };
    let pagination = match resource.storage {
        StorageKind::ControlRow => "crate::pagination::paginate",
        StorageKind::TenantData => "ryframe_db::pagination::paginate",
    };
    let model_trait = if resource.soft_delete.is_none() {
        "ModelTrait, "
    } else {
        ""
    };
    let transaction_trait = if resource.storage == StorageKind::ControlRow {
        "TransactionTrait, "
    } else {
        ""
    };
    let control_transaction_methods = control_transaction_methods(resource);
    format!(
        include_str!("repository.rs.tpl"),
        declaration = persistence.declaration,
        read_connection = persistence.read_connection,
        begin_transaction = persistence.begin_transaction,
        pagination = pagination,
        generic_conflict = generic_conflict,
        header = header,
        name = name,
        pascal = pascal,
        detail_import = detail_import,
        model_trait = model_trait,
        transaction_trait = transaction_trait,
        tenant_mismatch = tenant_mismatch,
        detail_type = detail_type,
        detail_read = detail_read,
        base_select = base_select,
        filters = filters,
        order = order,
        control_transaction_methods = control_transaction_methods,
        id_query = id_query,
        unique_conflicts = unique_conflicts,
        delete = delete,
        commit = commit,
        to_record = to_record,
        to_entity = to_entity,
        relation_mappers = relation_mappers,
        tenant_error = tenant_error,
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
    query.push_str(&format!(
        "\n                .filter(crate::generated::{name}::entity::Column::TenantId.eq(tenant_id))",
        name = target.name
    ));
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

use super::super::ResourceIr;

fn insert_model(resource: &ResourceIr) -> String {
    if !resource.fields.iter().any(|field| field.database_default()) {
        return "entity::ActiveModel::from(to_entity(record))".into();
    }
    let fields = resource
        .fields
        .iter()
        .filter(|field| !field.database_default())
        .map(|field| {
            format!(
                "            {}: sea_orm::ActiveValue::Set(record.{}),",
                field.name, field.name
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("entity::ActiveModel {{\n{fields}\n            ..Default::default()\n        }}")
}

pub(super) fn insert_body(resource: &ResourceIr, conflicts: &str, conflict: &str) -> String {
    let model = insert_model(resource);
    if resource.id_generation == crate::IdGeneration::AutoIncrement {
        let (tenant_capture, tenant_filter) = if resource.access.system_tenant_only {
            ("", "")
        } else {
            (
                "        let tenant_id = record.tenant_id.clone();\n",
                "            .filter(entity::Column::TenantId.eq(tenant_id))\n",
            )
        };
        // MySQL 只返回单个自增值，租户复合主键必须在同一事务中按实际 ID 回读。
        format!(
            "{tenant_capture}        let mut active = {model};\n        active.id = sea_orm::ActiveValue::NotSet;\n        entity::Entity::insert(active)\n            .exec_without_returning(&self.transaction)\n            .await\n            .db_conflicts(&[{conflicts}], {conflict:?})?;\n        entity::Entity::find()\n{tenant_filter}            .filter(sea_orm::ExprTrait::eq(sea_orm::sea_query::Expr::col(entity::Column::Id), sea_orm::sea_query::Expr::cust(\"LAST_INSERT_ID()\")))\n            .one(&self.transaction)\n            .await\n            .db()?\n            .map(to_record)\n            .ok_or_else(|| AppError::Database(\"自增记录插入后读取失败；事务尚未提交\".into()))"
        )
    } else {
        format!(
            "        {model}\n            .insert(&self.transaction)\n            .await\n            .map(to_record)\n            .db_conflicts(&[{conflicts}], {conflict:?})"
        )
    }
}

pub(super) fn update_body(resource: &ResourceIr) -> String {
    let omissions = resource
        .fields
        .iter()
        .filter(|field| field.database_update())
        .map(|field| {
            format!(
                "        active.{} = sea_orm::ActiveValue::NotSet;\n",
                field.name
            )
        })
        .collect::<String>();
    if omissions.is_empty() {
        return "        entity::ActiveModel::from(to_entity(record))\n            .reset_all()\n            .update(&self.transaction)".into();
    }
    format!(
        "        let mut active = entity::ActiveModel::from(to_entity(record)).reset_all();\n{omissions}        active.update(&self.transaction)"
    )
}

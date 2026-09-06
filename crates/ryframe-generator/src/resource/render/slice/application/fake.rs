use super::super::{ResourceIr, StorageKind, ValueType, rust_literal, uses_partial_text_filter};
use super::{business_index_fields, method_arguments, unique_business_indexes, unique_method_name};

pub(crate) fn fake(resource: &ResourceIr, header: &str) -> String {
    let pascal = &resource.pascal_name;
    let detail_type = if resource.relations.is_empty() {
        format!("{pascal}Record")
    } else {
        format!("{pascal}Detail")
    };
    let detail_import = if resource.relations.is_empty() {
        String::new()
    } else {
        format!(", {pascal}Detail")
    };
    let detail_mapping = if resource.relations.is_empty() {
        String::new()
    } else {
        let relation_fields = resource
            .relations
            .iter()
            .map(|relation| format!("                {}: None,", relation.name))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            ".map(|record| {pascal}Detail {{\n                record,\n{relation_fields}\n            }})"
        )
    };
    let tenant_mismatch = format!("{}事务租户不匹配", resource.labels.zh_cn);
    let collections = if resource.access.owner_field.is_some() {
        "BTreeMap, BTreeSet, VecDeque"
    } else {
        "BTreeMap, VecDeque"
    };
    let (active_record_filter_method, active_record_filter) = fake_active_record_filters(resource);
    let record_order = fake_record_order(resource);
    let filters = fake_filters(resource);
    let owner_filter = resource.access.owner_field.as_ref().map(|field| {
        let owner = resource.fields.iter().find(|candidate| candidate.name == *field).expect("数据范围字段已由 IR 校验");
        if owner.nullable {
            format!("            if !record.{field}.is_some_and(|owner_id| owner_visible(owner_id, filter.data_scope, &state.owner_departments)) {{ return None; }}")
        } else {
            format!("            if !owner_visible(record.{field}, filter.data_scope, &state.owner_departments) {{ return None; }}")
        }
    }).unwrap_or_default();
    let owner_helper = resource.access.owner_field.as_ref().map(|_| {
        "\nfn owner_visible(\n    owner_id: i64,\n    scope: &ryframe_kernel::DataScopeContext,\n    departments: &BTreeMap<i64, (Option<i64>, BTreeSet<i64>)>,\n) -> bool {\n    let department = departments.get(&owner_id);\n    match scope.scope {\n        ryframe_kernel::DataScope::All => true,\n        ryframe_kernel::DataScope::SelfOnly => owner_id == scope.user_id,\n        ryframe_kernel::DataScope::Dept => scope.dept_id.is_some_and(|dept_id| {\n            department.is_some_and(|(owner_dept, _)| *owner_dept == Some(dept_id))\n        }),\n        ryframe_kernel::DataScope::DeptAndChildren => scope.dept_id.is_some_and(|dept_id| {\n            department.is_some_and(|(owner_dept, ancestors)| {\n                *owner_dept == Some(dept_id) || ancestors.contains(&dept_id)\n            })\n        }),\n        ryframe_kernel::DataScope::Custom => {\n            (scope.include_self && owner_id == scope.user_id)\n                || department.is_some_and(|(owner_dept, _)| {\n                    owner_dept.is_some_and(|dept_id| scope.custom_dept_ids.contains(&dept_id))\n                })\n        }\n    }\n}\n"
    }).unwrap_or_default();
    let owner_state = resource
        .access
        .owner_field
        .as_ref()
        .map(|_| "    owner_departments: BTreeMap<i64, (Option<i64>, BTreeSet<i64>)>,\n")
        .unwrap_or_default();
    let owner_setter = resource.access.owner_field.as_ref().map(|_| {
        "\n    pub fn set_owner_department(\n        &self,\n        owner_id: i64,\n        department_id: Option<i64>,\n        ancestor_ids: impl IntoIterator<Item = i64>,\n    ) {\n        self.lock().owner_departments.insert(\n            owner_id,\n            (department_id, ancestor_ids.into_iter().collect()),\n        );\n    }\n"
    }).unwrap_or_default();
    let (control_call_variants, control_failure_variants, control_methods) =
        fake_control_parts(resource);
    format!(
        include_str!("fake.rs.tpl"),
        header = header,
        collections = collections,
        pascal = pascal,
        detail_import = detail_import,
        control_call_variants = control_call_variants,
        control_failure_variants = control_failure_variants,
        owner_state = owner_state,
        owner_setter = owner_setter,
        owner_helper = owner_helper,
        detail_type = detail_type,
        active_record_filter_method = active_record_filter_method,
        detail_mapping = detail_mapping,
        active_record_filter = active_record_filter,
        owner_filter = owner_filter,
        filters = filters,
        record_order = record_order,
        tenant_mismatch = tenant_mismatch,
        control_methods = control_methods,
    )
}
fn fake_active_record_filters(resource: &ResourceIr) -> (String, String) {
    let Some(soft_delete) = &resource.soft_delete else {
        return (String::new(), String::new());
    };
    let field = resource
        .fields
        .iter()
        .find(|field| field.name == soft_delete.field)
        .expect("软删除字段已经在 Resource IR 中校验");
    let active = match (&soft_delete.active, field.value_type) {
        (toml::Value::String(value), ValueType::String) => format!("{value:?}"),
        _ => rust_literal(&soft_delete.active, field.value_type),
    };
    (
        format!(
            "\n            .filter(|record| record.{} == {active})",
            field.name
        ),
        format!(
            "            if record.{} != {active} {{ return None; }}",
            field.name
        ),
    )
}

fn fake_record_order(resource: &ResourceIr) -> String {
    let field = resource
        .fields
        .iter()
        .find(|field| field.usage.sort && field.name != "tenant_id")
        .map(|field| (field.name.as_str(), field.usage.sort_desc))
        .unwrap_or(("id", false));
    if field.0 == "id" {
        if field.1 {
            "        records.sort_by_key(|record| std::cmp::Reverse(record.id));".to_owned()
        } else {
            "        records.sort_by_key(|record| record.id);".to_owned()
        }
    } else {
        if field.1 {
            format!(
                "        records.sort_by(|left, right| {{\n            right.{field}\n                .cmp(&left.{field})\n                .then_with(|| right.id.cmp(&left.id))\n        }});",
                field = field.0,
            )
        } else {
            format!(
                "        records.sort_by(|left, right| {{\n            left.{field}\n                .cmp(&right.{field})\n                .then_with(|| left.id.cmp(&right.id))\n        }});",
                field = field.0,
            )
        }
    }
}
fn fake_control_parts(resource: &ResourceIr) -> (String, String, String) {
    if resource.storage != StorageKind::ControlRow {
        return (String::new(), String::new(), String::new());
    }
    let pascal = &resource.pascal_name;
    let mut calls = String::new();
    let mut failures = String::new();
    let mut methods = String::new();
    if resource.configuration_versioned {
        calls.push_str("    LockConfiguration { tenant_id: String },");
        failures.push_str("    LockConfiguration,");
        methods.push_str(
            "    async fn lock_configuration(&self, tenant_id: &str) -> AppResult<()> {\n        self.ensure_tenant(tenant_id)?;\n        let mut state = self.lock_state();\n        state.calls.push(",
        );
        methods.push_str(&format!(
            "{pascal}Call::LockConfiguration {{ tenant_id: tenant_id.into() }});\n        fail_if_requested(&mut state, {pascal}Failure::LockConfiguration)\n    }}\n\n"
        ));
    }
    for index in unique_business_indexes(resource) {
        let fields = business_index_fields(resource, index);
        let method = unique_method_name(index);
        let variant = crate::naming::to_pascal_case(&method);
        let variant = variant.trim_end_matches("ForUpdate");
        let call_fields = fields
            .iter()
            .map(|field| {
                let ty = if field.value_type == ValueType::String {
                    "String"
                } else {
                    field.rust_type.as_str()
                };
                format!("{}: {ty}", field.name)
            })
            .collect::<Vec<_>>()
            .join(", ");
        calls.push_str(&format!(
            "\n    {variant} {{ tenant_id: String, {call_fields}, exclude_id: Option<i64> }},"
        ));
        failures.push_str(&format!("\n    {variant},"));
        let arguments = method_arguments(&fields);
        let call_values = fields
            .iter()
            .map(|field| {
                if field.value_type == ValueType::String {
                    format!("{}: {}.into()", field.name, field.name)
                } else {
                    format!("{}: {}", field.name, field.name)
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        let comparisons = fields
            .iter()
            .map(|field| format!("record.{0} == {0}", field.name))
            .collect::<Vec<_>>()
            .join(" && ");
        methods.push_str(&format!(
            "    async fn {method}(\n        &self,\n        tenant_id: &str,\n{arguments}        exclude_id: Option<i64>,\n    ) -> AppResult<Option<{pascal}Record>> {{\n        self.ensure_tenant(tenant_id)?;\n        {{\n            let mut state = self.lock_state();\n            state.calls.push({pascal}Call::{variant} {{ tenant_id: tenant_id.into(), {call_values}, exclude_id }});\n            fail_if_requested(&mut state, {pascal}Failure::{variant})?;\n        }}\n        Ok(self.lock_view().values().find(|record| exclude_id != Some(record.id) && {comparisons}).cloned())\n    }}\n\n"
        ));
    }
    if resource.configuration_versioned {
        calls.push_str("\n    IncrementConfigurationVersion { tenant_id: String },");
        failures.push_str("\n    IncrementConfigurationVersion,");
        methods.push_str(&format!(
            "    async fn increment_configuration_version(&self, tenant_id: &str) -> AppResult<()> {{\n        self.ensure_tenant(tenant_id)?;\n        let mut state = self.lock_state();\n        state.calls.push({pascal}Call::IncrementConfigurationVersion {{ tenant_id: tenant_id.into() }});\n        fail_if_requested(&mut state, {pascal}Failure::IncrementConfigurationVersion)\n    }}\n"
        ));
    }
    (calls, failures, methods)
}

fn fake_filters(resource: &ResourceIr) -> String {
    resource
        .fields
        .iter()
        .filter(|field| field.usage.filter && field.name != "tenant_id")
        .map(|field| {
            let value = if field.value_type == ValueType::String {
                format!(
                    "filter.{0}.filter(|value| !value.is_empty())",
                    field.name
                )
            } else {
                format!("filter.{}", field.name)
            };
            let mismatch = if uses_partial_text_filter(field) {
                if field.nullable {
                    format!(
                        "!record.{0}.as_deref().is_some_and(|candidate| candidate.contains(value))",
                        field.name
                    )
                } else {
                    format!("!record.{}.contains(value)", field.name)
                }
            } else if field.nullable {
                if field.value_type == ValueType::String {
                    format!("record.{}.as_deref() != Some(value)", field.name)
                } else {
                    format!("record.{} != Some(value)", field.name)
                }
            } else {
                format!("record.{} != value", field.name)
            };
            format!(
                "            if let Some(value) = {value}\n                && {mismatch}\n            {{\n                return None;\n            }}"
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use regex::Regex;

use super::{business, digest};
use crate::Result;

pub(crate) type Columns = BTreeMap<String, String>;
pub(crate) type Catalog = BTreeMap<String, Columns>;
pub(crate) type OrderedCatalog = BTreeMap<String, Vec<(String, String)>>;

pub(crate) struct Loaded {
    pub(crate) control: Catalog,
    pub(crate) tenant: Catalog,
    pub(crate) sources: Vec<PathBuf>,
}

pub(crate) struct OrderedLoaded {
    pub(crate) control: OrderedCatalog,
    pub(crate) tenant: OrderedCatalog,
    pub(crate) full: Option<(OrderedCatalog, OrderedCatalog)>,
    pub(crate) sources: Vec<PathBuf>,
}

pub(crate) fn excluded_tables() -> Vec<&'static str> {
    let mut values = EXCLUDED.to_vec();
    values.sort_unstable();
    values
}

static CONTROL_TABLES: &[&str] = &[
    "sys_tenant",
    "sys_cache_namespace_version",
    "sys_dept",
    "sys_user",
    "password_reset_requests",
    "sys_role",
    "sys_permission",
    "sys_menu",
    "sys_post",
    "sys_config",
    "sys_dict_type",
    "sys_dict_data",
    "sys_notice",
    "sys_oper_log",
    "sys_login_info",
    "sys_user_role",
    "sys_role_permission",
    "sys_role_dept",
    "sys_file",
    "sys_background_job",
    "sys_job_schedule",
    "sys_job_schedule_execution",
    "sys_message",
    "sys_message_audience",
    "sys_message_recipient",
    "sys_data_retention_run",
    "sys_user_import_job",
    "sys_user_import_row_result",
    "sys_tenant_config_bundle",
    "sys_tenant_config_transfer",
    "sys_tenant_config_transfer_item",
    "sys_product_plan",
    "sys_product_plan_version",
    "sys_product_plan_capability",
    "sys_tenant_product_plan",
    "sys_tenant_capability_override",
    "sys_tenant_provision_request",
    "sys_tenant_operation_lease",
    "sys_tenant_data_placement",
    "sys_tenant_data_migration",
    "sys_tenant_data_migration_item",
    "sys_background_job_attempt",
    "sys_outbox_event",
    "sys_export_job",
];

static EXCLUDED: &[&str] = &[
    "ryframe_resource_ownership",
    "sys_backup_set",
    "sys_backup_resource",
    "sys_restore_run",
    "sys_tenant_data_backup_point",
    "seaql_migrations",
];

static REVIEWED_TENANT: &[&str] = &[
    "biz_tenant_fence",
    "biz_tenant_target_slot",
    "ryframe_resource_ownership",
    "biz_data_scope_projection",
    "biz_data_scope_owner",
    "biz_data_scope_dept",
];

pub(crate) fn load(root: &Path) -> Result<Loaded> {
    let loaded = load_ordered(root, false)?;
    Ok(Loaded {
        control: into_catalog(loaded.control),
        tenant: into_catalog(loaded.tenant),
        sources: loaded.sources,
    })
}

pub(crate) fn load_ordered(root: &Path, include_full: bool) -> Result<OrderedLoaded> {
    let sql_path = root.join("sql/ryframe_config.sql");
    let generated_path = root.join("crates/ryframe-tenant-db/src/generated/catalog.rs");
    let baseline_path =
        root.join("crates/ryframe-tenant-db/src/migration/m20260820_000000_tenant_baseline.rs");
    let all_control = parse_ordered_schema(&read_text(&sql_path)?)?;
    let baseline = parse_ordered_schema(&read_text(&baseline_path)?)?;
    validate_catalogs(&all_control, &baseline)?;
    let control = all_control
        .iter()
        .filter(|(table, _)| !EXCLUDED.contains(&table.as_str()))
        .map(|(table, columns)| (table.clone(), columns.clone()))
        .collect();
    let (generated_tenant, mut generated_sources) =
        parse_generated(root, &read_text(&generated_path)?)?;
    let mut tenant = baseline
        .iter()
        .filter(|(table, _)| table.as_str() != "ryframe_resource_ownership")
        .map(|(table, columns)| (table.clone(), columns.clone()))
        .collect::<OrderedCatalog>();
    if tenant
        .keys()
        .any(|table| generated_tenant.contains_key(table))
    {
        return Err("生成租户表与固定基线表重复".into());
    }
    tenant.extend(generated_tenant);
    let mut business = business::load(root)?;
    if tenant
        .keys()
        .any(|table| business.columns.contains_key(table))
    {
        return Err("框架与独立业务目录重复声明租户表".into());
    }
    tenant.extend(business.ordered_columns()?);
    let mut sources = vec![sql_path, generated_path, baseline_path];
    sources.append(&mut generated_sources);
    sources.append(&mut business.sources);
    let full = if include_full {
        let mut full_tenant = baseline;
        full_tenant.extend(tenant.clone());
        if !business.columns.is_empty() {
            full_tenant.insert(
                "ryframe_business_migration".into(),
                business::ledger_column_pairs(root)?,
            );
            sources.push(root.join("crates/ryframe-sdk/src/migration/ledger.rs"));
        }
        Some((all_control, full_tenant))
    } else {
        None
    };
    Ok(OrderedLoaded {
        control,
        tenant,
        full,
        sources,
    })
}

fn validate_catalogs(control: &OrderedCatalog, baseline: &OrderedCatalog) -> Result<()> {
    if control
        .keys()
        .filter(|table| !EXCLUDED.contains(&table.as_str()))
        .cloned()
        .collect::<BTreeSet<_>>()
        != CONTROL_TABLES
            .iter()
            .map(|value| (*value).to_owned())
            .collect()
    {
        return Err("控制库表与已审查复制白名单不同；须先审查新增/删除表的状态及物理引用".into());
    }
    if baseline.keys().cloned().collect::<BTreeSet<_>>()
        != REVIEWED_TENANT
            .iter()
            .map(|value| (*value).to_owned())
            .collect()
    {
        return Err("租户基线含未审查表；须补齐复制白名单，不能过滤未知业务表".into());
    }
    Ok(())
}

pub(crate) fn parse_schema(content: &str) -> Result<Catalog> {
    Ok(into_catalog(parse_ordered_schema(content)?))
}

fn into_catalog(ordered: OrderedCatalog) -> Catalog {
    ordered
        .into_iter()
        .map(|(table, columns)| (table, columns.into_iter().collect()))
        .collect()
}

fn parse_ordered_schema(content: &str) -> Result<OrderedCatalog> {
    let table =
        Regex::new(r"(?s)CREATE TABLE(?: IF NOT EXISTS)? `([a-z0-9_]+)` \((.*?)\n\s*\)\s*ENGINE=")?;
    let column = Regex::new(r"(?m)^\s*`([a-z0-9_]+)`\s+([A-Za-z]+)")?;
    let mut result = OrderedCatalog::new();
    for part in table.captures_iter(content) {
        let name = &part[1];
        let mut columns = Vec::new();
        let mut names = BTreeSet::new();
        for capture in column.captures_iter(&part[2]) {
            if !names.insert(capture[1].to_owned()) {
                return Err("schema 表或列重复/缺失".into());
            }
            columns.push((capture[1].to_owned(), capture[2].to_ascii_uppercase()));
        }
        if columns.is_empty() || result.insert(name.to_owned(), columns).is_some() {
            return Err("schema 表或列重复/缺失".into());
        }
    }
    Ok(result)
}

fn parse_generated(root: &Path, content: &str) -> Result<(OrderedCatalog, Vec<PathBuf>)> {
    let descriptor = Regex::new(
        r#"(?s)TenantDataTableDescriptor \{\s*table: "([a-z0-9_]+)",.*?tenant_column: "([a-z0-9_]+)",.*?column_types: &\[(.*?)\],.*?has_generated_columns: (true|false),"#,
    )?;
    let empty = Regex::new(
        r#"(?s)GENERATED_TENANT_DATA_TABLES:\s*&\[[^\]]*TenantDataTableDescriptor\][^=]*=\s*&\[\s*\];"#,
    )?;
    let entries = descriptor.captures_iter(content).collect::<Vec<_>>();
    if empty.is_match(content) {
        if entries.is_empty() {
            return Ok((OrderedCatalog::new(), Vec::new()));
        }
        return Err("空生成租户目录包含描述符".into());
    }
    if entries.is_empty() {
        return Err("非空生成租户目录缺少可验证描述符".into());
    }
    let ddl_pattern = Regex::new(r##"(?s)pub const CREATE_TABLE_DDL: &str = r#"(.*?)"#;"##)?;
    let type_pattern = Regex::new(r#""([a-z]+)""#)?;
    let mut result = OrderedCatalog::new();
    let mut sources = Vec::new();
    for part in entries {
        let table = &part[1];
        if !table.starts_with("biz_")
            || &part[2] != "tenant_id"
            || &part[4] != "false"
            || result.contains_key(table)
        {
            return Err("生成租户描述符包含未审查表、重复表或不支持的列规则".into());
        }
        let resource = table.trim_start_matches("biz_");
        let generated = root.join("crates/ryframe-tenant-db/src/generated");
        let candidates = [
            generated.join(format!("business_{resource}_migration.rs")),
            generated.join(resource).join("migration.rs"),
        ];
        let migrations = candidates
            .iter()
            .filter(|path| path.is_file())
            .collect::<Vec<_>>();
        if migrations.len() != 1 {
            return Err("生成租户描述符缺少同名迁移".into());
        }
        let migration = read_text(migrations[0])?;
        let ddl = ddl_pattern
            .captures(&migration)
            .ok_or("生成租户迁移缺少确定性 DDL")?;
        let mut tables = parse_ordered_schema(&ddl[1])?;
        if tables.len() != 1 || !tables.contains_key(table) {
            return Err("生成租户迁移包含额外或错误表".into());
        }
        let columns = tables.remove(table).unwrap();
        let types = type_pattern
            .captures_iter(&part[3])
            .map(|capture| capture[1].to_owned())
            .collect::<Vec<_>>();
        if types.len() != columns.len()
            || columns
                .iter()
                .map(|(_, kind)| kind.to_ascii_lowercase())
                .ne(types)
        {
            return Err("生成租户描述符与迁移列类型不一致".into());
        }
        result.insert((*table).to_owned(), columns);
        sources.push(migrations[0].to_path_buf());
    }
    Ok((result, sources))
}

fn read_text(path: &Path) -> Result<String> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || digest::linked(&metadata) {
        return Err(format!("复制目录输入必须是普通文件：{}", path.display()).into());
    }
    String::from_utf8(fs::read(path)?)
        .map_err(|_| format!("复制目录输入必须是 UTF-8：{}", path.display()).into())
}

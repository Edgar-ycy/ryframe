use std::{collections::BTreeSet, fs, path::Path, sync::OnceLock};

use regex::Regex;
use toml::Value;

const CATALOG: &str = "crates/ryframe-tenant-db/src/generated/catalog.rs";
const REQUIRED: &[(&str, &[&str])] = &[
    (
        "crates/ryframe-tenant-db/src/generated/mod.rs",
        &["pub mod catalog;"],
    ),
    (
        "crates/ryframe-tenant-db/src/migration/catalog.rs",
        &[
            "tables: Cow::Borrowed(crate::generated::catalog::GENERATED_TENANT_DATA_TABLES)",
            "tenant_data_schema_fingerprint()",
            "self.validate_structure()?",
            "computed != expected",
            "Cow::Borrowed(_)",
            "super::business_catalog::runtime_fingerprint()",
        ],
    ),
    (
        "crates/ryframe-tenant-db/src/migration/business_catalog.rs",
        &[
            "OnceLock<RuntimeCatalog>",
            "TENANT_DATA_CATALOG.validate_structure()?",
            "GENERATED_TENANT_DATA_SCHEMA_FINGERPRINT",
            "catalog.validate_structure()?",
            "current.catalog != candidate.catalog",
            "foreign_key_dependencies",
        ],
    ),
    (
        "crates/ryframe-generator/src/resource/render/tenant_catalog.rs",
        &[
            "TenantDataTableDescriptor",
            "primary_key_cursor_columns",
            "checksum_columns",
            "column_types",
            "schema_canonical",
            "schema::column_type(field)",
        ],
    ),
    (
        "crates/ryframe-generator/src/resource/render/slice/migration.rs",
        &["super::super::schema::column_type(field)"],
    ),
];

pub(crate) fn validate_tenant_data_boundaries(root: &Path, errors: &mut Vec<String>) {
    let expected = expected_tables(root, errors);
    let catalog = read_required(root, CATALOG, errors);
    let mut actual = table_pattern()
        .captures_iter(&catalog)
        .filter_map(|capture| capture.get(1).map(|value| value.as_str().to_owned()))
        .collect::<Vec<_>>();
    actual.sort();
    if actual != expected {
        errors.push(format!(
            "租户数据复制目录与资源清单不一致：expected={expected:?}, actual={actual:?}"
        ));
    }
    for (relative, fragments) in REQUIRED {
        let source = read_required(root, relative, errors);
        for fragment in *fragments {
            if !source.contains(fragment) {
                errors.push(format!("租户数据生成或校验链路缺少 {fragment}：{relative}"));
            }
        }
    }
    validate_business_repositories(root, errors);
}

fn expected_tables(root: &Path, errors: &mut Vec<String>) -> Vec<String> {
    let directory = root.join("catalog/resources");
    let Ok(entries) = fs::read_dir(&directory) else {
        return Vec::new();
    };
    let mut expected = Vec::new();
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "toml")
        })
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        let source = fs::read_to_string(&path).unwrap_or_default();
        let Ok(Value::Table(resource)) = toml::from_str::<Value>(&source) else {
            errors.push(format!("无法解析租户资源清单：{}", relative(root, &path)));
            continue;
        };
        if resource
            .get("storage")
            .and_then(Value::as_table)
            .and_then(|storage| storage.get("kind"))
            .and_then(Value::as_str)
            != Some("tenant_data")
        {
            continue;
        }
        if let Some(table) = resource
            .get("database")
            .and_then(Value::as_table)
            .and_then(|database| database.get("table"))
            .and_then(Value::as_str)
        {
            expected.push(table.to_owned());
        }
    }
    expected.sort();
    expected
}

fn read_required(root: &Path, relative_path: &str, errors: &mut Vec<String>) -> String {
    fs::read_to_string(root.join(relative_path)).unwrap_or_else(|_| {
        errors.push(format!("租户数据边界所需源码缺失：{relative_path}"));
        String::new()
    })
}

fn validate_business_repositories(root: &Path, errors: &mut Vec<String>) {
    let generated = root.join("crates/ryframe-tenant-db/src/generated");
    let Ok(entries) = fs::read_dir(&generated) else {
        return;
    };
    let repositories = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("repository.rs"))
        .filter(|path| path.is_file())
        .collect::<BTreeSet<_>>();
    for path in repositories {
        let source = fs::read_to_string(&path).unwrap_or_default();
        if bypass_pattern().is_match(&source) {
            errors.push(format!(
                "租户业务 Repository 绕过受控会话：{}",
                relative(root, &path)
            ));
        }
    }
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn table_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r#"\btable:\s*"([a-zA-Z0-9_]+)""#).expect("固定正则必须有效"))
}

fn bypass_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(
            r"\b(?:ControlDatabaseCluster|TenantDataTargetHandle|TenantDatabaseTargetRegistry)\b|\.(?:write|source|open_target(?:_for_catalog)?|verify_target_now(?:_for_catalog)?|target_occupancy(?:_for_catalog)?|prepare_migration_target(?:_for_catalog)?|freeze_fence(?:_for_catalog)?|activate_fence(?:_for_catalog)?|delete_tenant_rows_batch(?:_for_catalog)?|runtime_snapshot|prepare_provisioning)\(",
        )
        .expect("固定正则必须有效")
    })
}

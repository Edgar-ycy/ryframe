//! 将正式恢复角色配置投影为固定 B0 能读取的私有 TOML 和子进程凭据。
use std::collections::BTreeMap;

use toml::{Table, Value};

use super::super::b0::PASSWORD_VARIABLES;
use crate::Result;

const MYSQL_TARGETS: [(&str, &str, &str); 3] = [
    ("shared", "shared", PASSWORD_VARIABLES[0]),
    ("dedicated-a", "dedicated", PASSWORD_VARIABLES[1]),
    ("dedicated-b", "dedicated", PASSWORD_VARIABLES[2]),
];

const MODERN_TELEMETRY_FIELDS: [&str; 7] = [
    "headers",
    "resource_attributes",
    "scheduled_delay_millis",
    "batch_export_timeout_secs",
    "max_concurrent_exports",
    "max_export_batch_size",
    "span_limits",
];

pub(super) struct LegacyConfig {
    pub(super) selected: String,
    pub(super) worker_id: i64,
    pub(super) passwords: BTreeMap<String, String>,
}

pub(super) fn project(mut merged: Table) -> Result<LegacyConfig> {
    let worker_id = merged
        .remove("snowflake_worker_id")
        .and_then(|value| value.as_integer())
        .filter(|id| (0..=1023).contains(id))
        .ok_or("B0 恢复角色实例编号必须是 0 到 1023 的整数")?;
    project_startup_fields(&mut merged)?;
    let passwords = project_targets(&mut merged)?;
    let selected = toml::to_string(&merged).map_err(|_| "B0 恢复配置无法安全序列化")?;
    Ok(LegacyConfig {
        selected,
        worker_id,
        passwords,
    })
}

fn project_startup_fields(merged: &mut Table) -> Result<()> {
    // B0 只启动已恢复的库。初始管理员密码属于现代显式初始化配置。
    if let Some(bootstrap) = merged.remove("bootstrap") {
        let bootstrap = bootstrap.as_table().ok_or("B0 来源 bootstrap 必须是表")?;
        if bootstrap
            .iter()
            .any(|(key, value)| key != "admin_password" || value.as_str().is_none())
        {
            return Err("B0 来源 bootstrap 字段不匹配".into());
        }
    }
    let app = merged
        .get_mut("app")
        .and_then(Value::as_table_mut)
        .ok_or("B0 来源 app 必须是表")?;
    // 固定 B0 在自己的执行根下读取 locales。
    if let Some(locales) = app.remove("locales_dir")
        && locales.as_str() != Some("locales")
    {
        return Err("B0 恢复配置要求使用原 locales 目录".into());
    }
    if let Some(telemetry) = merged.get_mut("telemetry") {
        let telemetry = telemetry
            .as_table_mut()
            .ok_or("B0 来源 telemetry 必须是表")?;
        if MODERN_TELEMETRY_FIELDS
            .iter()
            .any(|key| telemetry.contains_key(*key))
        {
            if telemetry.get("enabled").and_then(Value::as_bool) != Some(false) {
                return Err("B0 恢复只可移除已禁用的现代 telemetry 参数".into());
            }
            for key in MODERN_TELEMETRY_FIELDS {
                telemetry.remove(key);
            }
        }
    }
    Ok(())
}

fn project_targets(merged: &mut Table) -> Result<BTreeMap<String, String>> {
    let targets = merged
        .get_mut("tenant_data")
        .and_then(Value::as_table_mut)
        .and_then(|table| table.get_mut("targets"))
        .and_then(Value::as_array_mut)
        .filter(|targets| targets.len() == MYSQL_TARGETS.len())
        .ok_or("B0 恢复要求精确绑定三个租户 MySQL 目标")?;
    let mut passwords = BTreeMap::new();
    for target in targets {
        let target = target.as_table_mut().ok_or("B0 来源 MySQL 目标必须是表")?;
        let field = |key| target.get(key).and_then(Value::as_str);
        let (_, _, variable) = MYSQL_TARGETS
            .iter()
            .copied()
            .find(|(key, mode, _)| field("key") == Some(*key) && field("mode") == Some(*mode))
            .filter(|_| field("kind") == Some("mysql") && !target.contains_key("password_env"))
            .ok_or("B0 来源 MySQL 目标键、模式或凭据字段不匹配")?;
        let Some(Value::String(password)) = target.remove("password") else {
            return Err("B0 来源 MySQL 目标密码必须是字符串".into());
        };
        if password.contains('\0') || passwords.insert(variable.to_owned(), password).is_some() {
            return Err("B0 来源 MySQL 目标重复或凭据无法传给子进程".into());
        }
        target.insert("password_env".into(), Value::String(variable.to_owned()));
    }
    Ok(passwords)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Table {
        toml::from_str(
            r#"
scope_id = "test-base"
snowflake_worker_id = 1023
[app]
locales_dir = "locales"
port = 23921
[bootstrap]
[telemetry]
enabled = false
sample_ratio = 0.5
span_limits = { max_events_per_span = 4 }
[[tenant_data.targets]]
key = "dedicated-b"
kind = "mysql"
mode = "dedicated"
database = "b"
password = "fixture-value-b"
[[tenant_data.targets]]
key = "shared"
kind = "mysql"
mode = "shared"
database = "shared"
password = ""
[[tenant_data.targets]]
key = "dedicated-a"
kind = "mysql"
mode = "dedicated"
database = "a"
password = "fixture-value-a"
"#,
        )
        .unwrap()
    }

    #[test]
    fn projects_only_historical_fields_and_binds_passwords_by_target_key() {
        let projection = project(fixture()).unwrap();
        let selected = toml::from_str::<Table>(&projection.selected).unwrap();
        assert_eq!(projection.worker_id, 1023);
        assert_eq!(selected["scope_id"].as_str(), Some("test-base"));
        assert!(!selected.contains_key("snowflake_worker_id"));
        assert!(!selected.contains_key("bootstrap"));
        assert_eq!(selected["app"]["port"].as_integer(), Some(23921));
        assert!(selected["app"].get("locales_dir").is_none());
        assert_eq!(selected["telemetry"]["sample_ratio"].as_float(), Some(0.5));
        assert!(selected["telemetry"].get("span_limits").is_none());
        let targets = selected["tenant_data"]["targets"].as_array().unwrap();
        for (index, number, database, value) in [
            (0, 3, "b", "fixture-value-b"),
            (1, 1, "shared", ""),
            (2, 2, "a", "fixture-value-a"),
        ] {
            let name = format!("RYFRAME_B0_MYSQL_PASSWORD_{number}");
            assert_eq!(targets[index]["password_env"].as_str(), Some(name.as_str()));
            assert_eq!(targets[index]["database"].as_str(), Some(database));
            assert!(targets[index].get("password").is_none());
            assert_eq!(projection.passwords[&name], value);
        }
    }

    #[test]
    fn rejects_invalid_worker_id_duplicate_targets_and_active_unsupported_telemetry() {
        let mut merged = fixture();
        merged.insert("snowflake_worker_id".into(), Value::Integer(1024));
        assert!(project(merged).is_err());

        let mut merged = fixture();
        let targets = merged
            .get_mut("tenant_data")
            .unwrap()
            .as_table_mut()
            .unwrap()
            .get_mut("targets")
            .unwrap()
            .as_array_mut()
            .unwrap();
        targets[1] = targets[0].clone();
        assert!(project(merged).is_err());

        let mut merged = fixture();
        merged
            .get_mut("telemetry")
            .unwrap()
            .as_table_mut()
            .unwrap()
            .insert("enabled".into(), Value::Boolean(true));
        assert!(project(merged).is_err());
    }
}

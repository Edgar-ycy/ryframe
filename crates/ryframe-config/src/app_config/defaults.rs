use crate::{AppSettings, Environment};

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            name: "ryframe".into(),
            host: "0.0.0.0".into(),
            port: 8080,
        }
    }
}

pub(super) fn apply_migration_mode_default(table: &mut toml::Table, environment: Environment) {
    let Some(toml::Value::Table(database)) = table.get_mut("database") else {
        return;
    };
    database.entry("migration_mode").or_insert_with(|| {
        toml::Value::String(
            if environment.is_production() {
                "verify"
            } else {
                "auto"
            }
            .into(),
        )
    });
}

pub(super) fn apply_job_mode_default(table: &mut toml::Table, environment: Environment) {
    let jobs = table
        .entry("jobs")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let toml::Value::Table(jobs) = jobs else {
        return;
    };
    jobs.entry("mode").or_insert_with(|| {
        toml::Value::String(
            if environment.is_production() {
                "external"
            } else {
                "embedded"
            }
            .into(),
        )
    });
}

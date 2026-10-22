use std::path::PathBuf;

use crate::{AppSettings, Environment};

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            name: "ryframe".into(),
            host: "0.0.0.0".into(),
            port: 8080,
            locales_dir: default_locales_dir(),
        }
    }
}

pub(super) fn default_locales_dir() -> PathBuf {
    PathBuf::from("locales")
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

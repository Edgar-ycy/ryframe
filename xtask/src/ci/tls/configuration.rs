use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use super::integration_configuration::{
    IntegrationConfiguration, capture::Captured, files, owned::OwnedConfiguration,
};
use files::diagnostic;

pub(crate) struct Published {
    roles: Vec<OwnedConfiguration>,
    ca: Option<(PathBuf, files::Image)>,
    started: bool,
    complete: bool,
}

impl Published {
    pub(crate) fn new() -> Self {
        Self {
            roles: Vec::new(),
            ca: None,
            started: false,
            complete: false,
        }
    }

    pub(crate) fn publish(
        &mut self,
        root: &Path,
        input: &IntegrationConfiguration,
        ca: &Path,
        redis_port: u16,
        https_port: u16,
    ) -> Result<(), String> {
        input.verify()?;
        self.started = true;
        self.ca = Some((ca.to_owned(), files::read_required(ca)?));
        let parent = root.join(".local-tests/service-tests");
        files::create_parents(&parent)?;
        for role in ["redis-tls", "s3-https", "otlp-https"] {
            let role_parent = parent.join(role);
            files::create_parents(&role_parent)?;
            let directory = role_parent.join("config");
            let selected = selected_for_role(&input.redis, role, ca, redis_port, https_port)?;
            let mut content = input.redis.derived_files(&selected)?;
            if role.ends_with("https") {
                let metadata = toml::Table::from_iter([(
                    "ca_certificate".into(),
                    toml::Value::String(ca.to_string_lossy().into_owned()),
                )]);
                content.insert("test.toml".into(), encode(&metadata, &directory)?);
            }
            self.roles
                .push(OwnedConfiguration::create(&directory, &content)?);
            let captured = Captured::load(&directory, role.ends_with("https"))?;
            captured.verify()?;
        }
        self.verify(input)?;
        self.complete = true;
        Ok(())
    }

    pub(crate) fn verify(&self, input: &IntegrationConfiguration) -> Result<(), String> {
        input.verify()?;
        if let Some((path, expected)) = &self.ca
            && files::read_required(path)? != *expected
        {
            return Err(diagnostic(
                path,
                "复核fixture CA",
                "CA字节或身份元数据漂移，保留现场",
            ));
        }
        for role in &self.roles {
            role.verify()?;
        }
        Ok(())
    }

    pub(crate) fn clean(self, input: &IntegrationConfiguration) -> Vec<String> {
        if !self.started {
            return Vec::new();
        }
        if !self.complete {
            return vec![diagnostic(
                &input.redis.directory,
                "清理配置",
                "三角色发布未完整确认；已有/部分目录和证书保留，不自动重放",
            )];
        }
        if let Err(error) = self.verify(input) {
            return vec![error];
        }
        self.roles
            .into_iter()
            .filter_map(|role| role.remove().err())
            .collect()
    }
}

pub(crate) fn selected_for_role(
    source: &Captured,
    role: &str,
    ca: &Path,
    redis_port: u16,
    https_port: u16,
) -> Result<toml::Table, String> {
    let mut selected = source.selected.clone();
    let path = source.directory.join("app.test.toml");
    let mut redis = toml::Table::new();
    redis.insert("host".into(), "127.0.0.1".into());
    redis.insert("port".into(), i64::from(redis_port).into());
    redis.insert("tls".into(), true.into());
    redis.insert("tls_ca".into(), ca.to_string_lossy().into_owned().into());
    // 只覆盖这四个字段，其余账号、DB、mTLS、池、超时全部由捕获配置深合并保留。
    if role == "redis-tls" {
        merge_section(&mut selected, "redis", redis, &path)?;
    }
    if role == "s3-https" {
        merge_section(
            &mut selected,
            "object_storage",
            BTreeMap::from([
                ("backend".into(), "s3".into()),
                (
                    "endpoint".into(),
                    format!("https://127.0.0.1:{https_port}").into(),
                ),
                ("access_key".into(), "ryframe-integration-access".into()),
                ("secret_key".into(), "ryframe-integration-secret".into()),
                ("use_ssl".into(), true.into()),
                ("region".into(), "us-east-1".into()),
                ("request_timeout_secs".into(), 15_i64.into()),
            ])
            .into_iter()
            .collect(),
            &path,
        )?;
    }
    if role == "otlp-https" {
        merge_section(
            &mut selected,
            "telemetry",
            BTreeMap::from([
                ("enabled".into(), true.into()),
                (
                    "endpoint".into(),
                    format!("https://127.0.0.1:{https_port}/v1/traces").into(),
                ),
                ("service_name".into(), "ryframe-aws-lc-https-test".into()),
                ("sample_ratio".into(), 1.0.into()),
                ("export_timeout_secs".into(), 15_i64.into()),
                ("max_queue_size".into(), 16_i64.into()),
            ])
            .into_iter()
            .collect(),
            &path,
        )?;
    }
    Ok(selected)
}

fn merge_section(
    table: &mut toml::Table,
    name: &str,
    fields: toml::Table,
    path: &Path,
) -> Result<(), String> {
    let section = table
        .entry(name)
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .ok_or_else(|| diagnostic(path, "构造TLS角色配置", &format!("字段{name}必须为表")))?;
    section.extend(fields);
    Ok(())
}

fn encode(table: &toml::Table, path: &Path) -> Result<Vec<u8>, String> {
    toml::to_string(table)
        .map(String::into_bytes)
        .map_err(|_| diagnostic(path, "编码test.toml", "无法序列化CA元数据"))
}

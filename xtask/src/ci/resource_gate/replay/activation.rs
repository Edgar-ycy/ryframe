use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use ryframe_config::DbTlsMode;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::ci::integration_configuration::{
    capture::Captured,
    files::{self, Image, diagnostic},
    owned::OwnedConfiguration,
};

const MYSQL: &str = ".local-tests/mysql/config";
const REDIS: &str = ".local-tests/service-tests/redis/config";

// 不派生 Debug/Serialize：捕获中包含凭据，只发布路径和文件绑定。
pub(crate) struct Activation {
    mysql: Role,
    redis: Role,
}

struct Role {
    directory: PathBuf,
    captured: Option<Captured>,
    images: BTreeMap<PathBuf, Option<Image>>,
}

impl Activation {
    pub(super) fn load(root: &Path, required: bool) -> Result<Self, String> {
        let input = Self {
            mysql: Role::load(root.join(MYSQL), true)?,
            redis: Role::load(root.join(REDIS), false)?,
        };
        if required && (input.mysql.captured.is_none() || input.redis.captured.is_none()) {
            return Err(diagnostic(
                root,
                "activation配置",
                "必须提供MySQL和Redis两个真实测试角色目录及有效app.test.toml；旧环境标志无效",
            ));
        }
        input.verify()?;
        Ok(input)
    }

    pub(super) fn verify(&self) -> Result<(), String> {
        self.mysql.verify()?;
        self.redis.verify()
    }

    pub(super) fn record(
        &self,
        environment: &mut BTreeMap<String, String>,
    ) -> Result<Value, String> {
        self.verify()?;
        let mysql = if self.mysql.captured.is_some() {
            "1"
        } else {
            "0"
        };
        let redis = if self.redis.captured.is_some() {
            "1"
        } else {
            "0"
        };
        for (name, value) in [
            ("mysqlIntegration", mysql),
            ("mysqlTlsIntegration", mysql),
            ("redisIntegration", redis),
        ] {
            environment.insert(name.to_owned(), value.to_owned());
        }
        Ok(json!({"mysql":self.mysql.record(), "redis":self.redis.record()}))
    }

    pub(super) fn publish(&self, root: &Path) -> Result<Published, String> {
        self.verify()?;
        let mut roles = Vec::new();
        for (relative, role) in [(MYSQL, &self.mysql), (REDIS, &self.redis)] {
            let directory = root.join(relative);
            if let Some(captured) = &role.captured {
                files::create_parents(directory.parent().expect("固定角色路径"))?;
                let content = captured.derived_files(&captured.selected)?;
                roles.push((
                    directory.clone(),
                    Some(OwnedConfiguration::create(&directory, &content)?),
                ));
            } else {
                if files::directory_available(&directory)? {
                    return Err(diagnostic(
                        &directory,
                        "发布角色",
                        "来源缺失但隔离树角色已存在；禁止静默启用",
                    ));
                }
                roles.push((directory, None));
            }
        }
        let published = Published { roles };
        published.verify()?;
        self.verify()?;
        Ok(published)
    }
}

pub(super) struct Published {
    roles: Vec<(PathBuf, Option<OwnedConfiguration>)>,
}

impl Published {
    pub(super) fn verify(&self) -> Result<(), String> {
        for (directory, owned) in &self.roles {
            if let Some(owned) = owned {
                owned.verify()?;
            } else if files::directory_available(directory)? {
                return Err(diagnostic(
                    directory,
                    "复核缺失角色",
                    "原先缺失的角色目录已出现；保留现场，禁止重新捕获",
                ));
            }
        }
        Ok(())
    }
}

impl Role {
    fn load(directory: PathBuf, mysql: bool) -> Result<Self, String> {
        let mut role = Self {
            directory,
            captured: None,
            images: BTreeMap::new(),
        };
        if !files::directory_available(&role.directory)? {
            return Ok(role);
        }
        for name in ["app.toml", "app.test.toml"] {
            let path = role.directory.join(name);
            role.images
                .insert(path.clone(), files::read_optional(&path)?);
        }
        let captured = Captured::load(&role.directory, false)?;
        if mysql {
            let primary = &captured.app.database.primary;
            loopback(&role.directory, "database.primary.host", &primary.host)?;
            if primary.tls_mode == DbTlsMode::Disabled {
                return Err(diagnostic(
                    &role.directory,
                    "database.primary.tls_mode",
                    "MySQL真实协议测试必须启用TLS",
                ));
            }
        } else {
            let redis = captured
                .app
                .redis
                .as_ref()
                .ok_or_else(|| diagnostic(&role.directory, "redis", "缺少真实Redis角色配置"))?;
            if !captured.redis_endpoint_explicit {
                return Err(diagnostic(
                    &role.directory,
                    "redis.host/redis.port",
                    "真实测试必须显式给出host和port，不采用默认端点",
                ));
            }
            if redis.mode != ryframe_config::RedisMode::Required {
                return Err(diagnostic(
                    &role.directory,
                    "redis.mode",
                    "真实Redis测试必须为required",
                ));
            }
            loopback(&role.directory, "redis.host", &redis.host)?;
            if redis.database != 15 {
                return Err(diagnostic(
                    &role.directory,
                    "redis.database",
                    "replay要求隔离数据库15",
                ));
            }
        }
        // Captured已绑定Redis材料；数据库TLS材料也必须覆盖，不能仅核验路径字符串。
        let database = &captured.app.database;
        for connection in std::iter::once(&database.primary)
            .chain(database.replicas.iter().map(|item| &item.connection))
            .chain(database.sources.iter().map(|item| &item.connection))
        {
            for value in [
                &connection.tls_ca,
                &connection.tls_client_cert,
                &connection.tls_client_key,
            ]
            .into_iter()
            .flatten()
            {
                let path = PathBuf::from(value);
                if !path.is_absolute() {
                    return Err(diagnostic(
                        &role.directory,
                        "数据库TLS材料",
                        "必须是明确绝对文件路径",
                    ));
                }
                role.images
                    .insert(path.clone(), Some(files::read_required(&path)?));
            }
        }
        role.captured = Some(captured);
        role.verify()?;
        Ok(role)
    }

    fn verify(&self) -> Result<(), String> {
        if let Some(captured) = &self.captured {
            captured.verify()?;
            for (path, image) in &self.images {
                if files::read_optional(path)? != *image {
                    return Err(diagnostic(
                        path,
                        "复核replay来源",
                        "存在性、身份或内容漂移；保留现场，禁止重新捕获",
                    ));
                }
            }
        } else if files::directory_available(&self.directory)? {
            return Err(diagnostic(
                &self.directory,
                "复核缺失角色",
                "原先缺失的配置目录已出现；保留现场，禁止重新捕获",
            ));
        }
        Ok(())
    }

    fn record(&self) -> Value {
        let mut files = self
            .images
            .iter()
            .map(|(path, image)| binding(path, image.as_ref()))
            .collect::<Vec<_>>();
        if let Some(captured) = &self.captured {
            files.extend(
                captured
                    .materials
                    .iter()
                    .map(|(path, image)| binding(path, Some(image))),
            );
        } else {
            files.extend(
                ["app.toml", "app.test.toml"].map(|name| binding(&self.directory.join(name), None)),
            );
        }
        json!({"directory":self.directory,"present":self.captured.is_some(),"files":files})
    }
}

fn loopback(path: &Path, field: &str, host: &str) -> Result<(), String> {
    if !["127.0.0.1", "localhost", "::1"].contains(&host) {
        return Err(diagnostic(
            path,
            field,
            "只允许127.0.0.1、localhost、::1三个回环主机值",
        ));
    }
    Ok(())
}

fn binding(path: &Path, image: Option<&Image>) -> Value {
    match image {
        Some(image) => {
            json!({"path":path,"present":true,"bytes":image.bytes.len(),"sha256":format!("{:x}", Sha256::digest(&image.bytes))})
        }
        None => json!({"path":path,"present":false}),
    }
}

#[cfg(test)]
mod tests;

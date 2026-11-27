//! 来源验证的实际查询端口；策略、缓存事务及收据判定仍使用同一原生模型。
use std::{collections::BTreeMap, path::Path, time::Duration};

use serde_json::{Value, json};

use super::{binding, producer::launch, ready};
use crate::utc_timestamp::now_rfc3339;
use crate::{
    Result,
    recovery::clone::native::{
        export::transport::{CommandSpec, StdoutTarget, Transport},
        json as document,
        s3::app_configuration,
        target_verify::{
            input::{request, request_physical},
            redis::Client,
            storage::StorageInputs,
        },
    },
};

#[path = "resources/cache.rs"]
mod cache;
#[path = "resources/inventory.rs"]
mod inventory;
#[path = "resources/storage.rs"]
mod storage;
#[path = "resources/warm_expiry.rs"]
mod warm_expiry;
#[path = "resources/warm_redis.rs"]
pub(super) mod warm_redis;
#[path = "resources/warm_transient.rs"]
pub(super) mod warm_transient;

pub(super) struct Resources<'a, T> {
    pub backend: &'a Path,
    pub execution: &'a Path,
    pub request: &'a Value,
    pub generation: &'a Value,
    pub environment: &'a BTreeMap<String, String>,
    pub output: &'a Path,
    pub transport: &'a T,
    pub checkpoint: &'a dyn Fn() -> Result<()>,
}

impl<T: Transport> Resources<'_, T> {
    fn inputs(&self) -> StorageInputs<'_> {
        StorageInputs {
            backend: self.backend,
            request: self.request,
            environment: self.environment,
            observation_dir: self.output,
        }
    }

    pub(super) fn policy(&self, action: &str, arguments: Value) -> Result<Value> {
        ready::verify(&json!({"action":action,"arguments":arguments}))
    }

    pub(super) fn mysql(&self, database: &Value, sql: &str) -> Result<String> {
        (self.checkpoint)()?;
        request_physical::defaults_connection(self.backend, database)?;
        let executable = request::external_file(&self.request["tools"]["mysql"])?;
        let arguments = vec![
            format!(
                "--defaults-file={}",
                document::text(&database["defaults_file"], "MySQL 配置文件")?
            ),
            "--connect-timeout=5".into(),
            "--batch".into(),
            "--raw".into(),
            "--skip-column-names".into(),
            "--binary-mode".into(),
            "--default-character-set=utf8mb4".into(),
        ];
        let mut command = vec![executable.to_string_lossy().into_owned()];
        command.extend(arguments.clone());
        let mut environment = self.environment.clone();
        environment.remove("MYSQL_PWD");
        environment.insert(
            "MYSQL_TEST_LOGIN_FILE".into(),
            self.output
                .join("unused-login.cnf")
                .to_string_lossy()
                .into_owned(),
        );
        let result = self.transport.run(CommandSpec {
            program: executable,
            args: arguments,
            cwd: self.execution.to_owned(),
            environment,
            stdin: Some(sql.as_bytes().to_vec()),
            stdout: StdoutTarget::Capture,
            work_dir: self.output.to_owned(),
            label: "source-login-audit".into(),
            timeout: Duration::from_secs(30),
        });
        let stdout = result
            .as_ref()
            .map(|output| output.stdout.as_slice())
            .unwrap_or_default();
        let stderr = match &result {
            Ok(output) => self.redact(&String::from_utf8_lossy(&output.stderr))?,
            Err(error) => self.redact(&format!("固定只读 MySQL 进程调用失败：{error}"))?,
        };
        let receipt = json!({"command":command,"returncode":result.as_ref().ok().and_then(|output|output.status.code()),
            "error_type":if result.is_err(){json!("TransportError")}else{Value::Null},
            "stdout":self.redact(&String::from_utf8_lossy(stdout))?,"stderr":stderr});
        let key = document::text(&database["key"], "来源审计数据库 key")?;
        let receipt_path = self.output.join(format!(
            "mysql-{key}-{}.command.json",
            uuid::Uuid::new_v4().simple()
        ));
        document::write_new(&receipt_path, &receipt)?;
        (self.checkpoint)()?;
        request_physical::defaults_connection(self.backend, database)?;
        let output = result.map_err(|_| mysql_failure(&receipt_path, "进程调用未确认完成"))?;
        if !output.status.success() {
            return Err(mysql_failure(&receipt_path, "进程非零退出").into());
        }
        Ok(std::str::from_utf8(&output.stdout)
            .map_err(|_| "来源 MySQL 审计响应不是 UTF-8")?
            .trim()
            .to_owned())
    }

    fn redact(&self, text: &str) -> Result<Value> {
        let value = binding::redact(
            &json!({"backend":self.backend,"databases":self.request["target"]["databases"],"text":text}),
        )?;
        let configuration = if self.generation["format_version"].as_u64() == Some(2) {
            super::generation_request::configuration::verify(self.backend, self.generation)?
        } else {
            let directory = request::config_directory(self.backend, self.request)?;
            app_configuration::configuration_value(
                self.execution,
                Path::new(document::text(&directory["path"], "历史来源绑定配置目录")?),
                "test",
            )?
        };
        let objects = app_configuration::private_configuration(self.backend, &configuration)?;
        let mut text = document::text(&value, "安全 MySQL 诊断")?.to_owned();
        for field in ["access_key", "secret_key"] {
            let secret = document::text(&objects[field], "绑定对象配置")?;
            text = text.replace(secret, "[REDACTED]");
        }
        for value in self.environment.values().filter(|value| !value.is_empty()) {
            text = text.replace(value, "[REDACTED]");
        }
        Ok(json!(text))
    }

    fn redis_window<R>(
        &self,
        generation: &Value,
        read: impl FnOnce(&mut dyn FnMut(&[&str]) -> Result<Value>) -> Result<R>,
    ) -> Result<R> {
        let original = &self.request["storage"]["redis"];
        if original["platform"] != "windows" || generation["platform"] != "windows" {
            return Err("来源缓存读取需要原审阅和 START 绑定的 Windows Redis 代次".into());
        }
        (self.checkpoint)()?;
        let result = Client::authenticated(&self.inputs(), generation).and_then(|mut client| {
            let result = read(&mut |parts| {
                (self.checkpoint)()?;
                let result = client.command(parts);
                checked_redis_read(result, (self.checkpoint)())
            });
            #[cfg(windows)]
            let result = checked_redis_read(result, client.recheck_windows_peer());
            result
        });
        checked_redis_read(result, (self.checkpoint)())
    }

    pub(super) fn login_before(&self) -> Result<(Vec<u8>, u64)> {
        let raw = self.login_query("all", Value::Null)?;
        let evidence = self.policy("login-row-evidence", json!({"raw":raw}))?;
        let bytes = hex::decode(document::text(&evidence["hex"], "登录前像字节")?)?;
        launch::write_bytes(&self.output.join("login-before.tsv"), &bytes)?;
        Ok((
            bytes,
            evidence["upper_id"].as_u64().ok_or("登录前像上界无效")?,
        ))
    }

    fn login_query(&self, selection: &str, upper: Value) -> Result<String> {
        let database = self.policy(
            "control-database",
            json!({"databases":self.request["target"]["databases"]}),
        )?;
        let sql = self.policy(
            "login-query",
            json!({"database":database["database"],"selection":selection,"upper_id":upper}),
        )?;
        self.mysql(&database, document::text(&sql, "来源固定审计查询")?)
    }

    pub(super) fn login_after(
        &self,
        before: &[u8],
        upper: u64,
        lineage: &Value,
        subjects: &Value,
        started: &str,
    ) -> Result<Value> {
        let old = self.login_query("old", json!(upper))?;
        let added = self.login_query("new", json!(upper))?;
        let old = terminated(&old);
        launch::write_bytes(&self.output.join("login-after-old.tsv"), &old)?;
        launch::write_bytes(
            &self.output.join("login-after-new.tsv"),
            &terminated(&added),
        )?;
        let directory = self
            .output
            .parent()
            .ok_or("来源审计缺少 verification 目录")?;
        let value = self.policy("login-create",json!({"backend":self.backend,"directory":directory,
            "old_before":hex::encode(before),"old_after":hex::encode(&old),"upper_id":upper,"new_raw":added,
            "lineage":lineage,"subjects":subjects,"started_at":started,"completed_at":now_rfc3339()}))?;
        document::write_new(&self.output.join("login-audit.json"), &value)?;
        Ok(value)
    }

    pub(super) fn recheck_login(&self, upper: u64) -> Result<()> {
        let directory = self
            .output
            .parent()
            .ok_or("恢复审计缺少 verification 目录")?;
        for (selection, filename) in [
            ("old", "login-after-old.tsv"),
            ("new", "login-after-new.tsv"),
        ] {
            let raw = self.login_query(selection, json!(upper))?;
            self.policy(
                "login-recheck",
                json!({"backend":self.backend,"directory":directory,"filename":filename,"raw":raw}),
            )?;
        }
        Ok(())
    }
}

fn mysql_failure(receipt: &Path, cause: &str) -> String {
    format!(
        "来源登录审计固定只读查询失败：{cause}；已脱敏原因见收据 {}。本阶段未授权远端写入；检查收据中的工具路径、绑定 defaults-file、连接可达性和查询权限，保留现场，不自动重试",
        receipt.display()
    )
}

fn terminated(text: &str) -> Vec<u8> {
    if text.is_empty() {
        Vec::new()
    } else {
        format!("{text}\n").into_bytes()
    }
}

fn checked_redis_read<R>(result: Result<R>, checked: Result<()>) -> Result<R> {
    match (result, checked) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(after)) => Err(format!("{error}；来源缓存读取后验失败：{after}").into()),
    }
}

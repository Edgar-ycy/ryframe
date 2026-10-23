//! 在同一 Rust 调用内冻结配置，秘密快照不经过 Python 或公开收据。
use super::{files, model::Input, prepare};
use crate::{Result, devex::runtime_native::process::Cancellation};
use serde_json::Value;
use std::path::Path;

pub(crate) struct Frozen(Input);

impl Frozen {
    pub(crate) fn capture(
        root: &Path,
        output: &Path,
        run: &str,
        attempt: &str,
        container: &str,
    ) -> Result<Self> {
        let directory = root.join(".local-tests/full-stack-config");
        let read = |name| -> Result<String> {
            String::from_utf8(files::read(&directory.join(name), 1024 * 1024)?)
                .map_err(|_| "CI 配置不是 UTF-8".into())
        };
        let common_text = read("app.toml")?;
        let environment_text = read("app.test.toml")?;
        let table = crate::rate_limit::load_app_table(root, Some(&directory), "test")
            .map_err(|_| "CI 配置加载失败；检查 app.test.toml，不回显配置内容")?;
        if common_text != read("app.toml")? || environment_text != read("app.test.toml")? {
            return Err("CI 配置在冻结期间漂移；未执行写入".into());
        }
        let value = Input {
            format_version: 1,
            backend: root.into(),
            output: output.into(),
            config_dir: directory,
            github_actions: "true".into(),
            run_id: run.into(),
            attempt: attempt.into(),
            app_env: "test".into(),
            container: container.into(),
            settings: serde_json::to_value(table)?,
            common_text,
            environment_text,
        };
        value.validate()?;
        Ok(Self(value))
    }

    pub(crate) fn settings(&self) -> &Value {
        &self.0.settings
    }

    pub(crate) fn scope(&self) -> Result<String> {
        Ok(self.0.settings["scope_id"]
            .as_str()
            .ok_or("缺少冻结 scope")?
            .into())
    }

    pub(crate) async fn prepare(&self, cancel: &Cancellation) -> Result<Value> {
        let plan = self.0.validate()?;
        prepare::execute(&plan, cancel).await.map_err(|code| {
            format!("CI 数据库阶段 {code}；核对 database-targets.json 和四库前后像，禁止自动重放")
                .into()
        })
    }
}

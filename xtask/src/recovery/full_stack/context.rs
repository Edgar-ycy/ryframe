use crate::{
    Result,
    ci::databases::snapshot::Frozen,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::clone::native::json as documents,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(crate) struct Context {
    pub root: PathBuf,
    pub output: PathBuf,
    pub control: Value,
    pub run: String,
    pub attempt: String,
}

impl Context {
    pub fn load(root: &Path) -> Result<Self> {
        if std::env::var("APP_ENV").as_deref() != Ok("test") {
            return Err("全栈协调只接受 APP_ENV=test；请显式选择隔离文件配置".into());
        }
        let control = crate::ci::databases::load_control(root)?;
        let output = PathBuf::from(control["runtime_dir"].as_str().ok_or("缺少运行目录")?);
        Ok(Self {
            root: root.into(),
            output,
            control,
            run: std::env::var("GITHUB_RUN_ID").unwrap_or_default(),
            attempt: std::env::var("GITHUB_RUN_ATTEMPT").unwrap_or_default(),
        })
    }

    pub fn frozen(&self) -> Result<Frozen> {
        if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
            return Err("CI prepare 只接受当前 GitHub Job".into());
        }
        let target = crate::recovery::clone::native::bridge::cargo_environment::configured_target(
            &self.root,
        )?
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from))
        .ok_or(
            "缺少 Cargo 目标目录；在工具文件配置 generator.target_dir，或核对 CI 的 Cargo 标准环境",
        )?;
        validate_absolute_path(&target, LocalTestPathKind::StateDirectory)?;
        Frozen::capture(
            &self.root,
            &self.output,
            &self.run,
            &self.attempt,
            self.container("mysql")?,
        )
    }

    pub fn container(&self, role: &str) -> Result<&str> {
        self.control[format!("{role}_container_id")]
            .as_str()
            .ok_or_else(|| format!("当前 Job 缺少 {role} 容器绑定").into())
    }

    pub fn table(&self) -> Result<Value> {
        crate::rate_limit::load_app_table(&self.root, Some(&self.output.join("config")), "test")
            .map_err(|_| "无法读取已绑定全栈配置，不回显配置行".into())
            .and_then(|table| Ok(serde_json::to_value(table)?))
    }

    pub fn api_url(&self) -> Result<String> {
        api_url(&self.table()?)
    }

    pub fn rate_limit(&self, path: &Path) -> Result<()> {
        validate_absolute_path(path, LocalTestPathKind::OutputFile)?;
        let authority =
            crate::rate_limit::authority(&self.root, Some(&self.output.join("config")), "test")
                .map_err(|_| "登录预算配置无效，未发布工具文件")?;
        let authority = serde_json::to_value(authority)?;
        let capacity = authority["login"]["capacity"]
            .as_u64()
            .filter(|v| (1..=10000).contains(v))
            .ok_or("登录容量无效")?;
        let window = authority["login"]["window_ms"]
            .as_u64()
            .filter(|v| (1000..=86400000).contains(v) && v % 1000 == 0)
            .ok_or("登录窗口无效")?;
        documents::write_new(
            path,
            &json!({"loginBudgetState":self.output.join("login-budget.json"),"loginCapacity":capacity,"loginWindowSeconds":window/1000}),
        )?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        println!("已从绑定配置写入浏览器登录预算结构字段");
        Ok(())
    }
}

pub(super) fn api_url(table: &Value) -> Result<String> {
    let port = table["app"]["port"]
        .as_u64()
        .filter(|v| (1..=65535).contains(v))
        .ok_or("API 端口无效")?;
    if table["app"]["host"] != "127.0.0.1" {
        return Err("API 必须绑定本机 IPv4".into());
    }
    Ok(format!("http://127.0.0.1:{port}/readyz"))
}

pub(super) fn journal(path: &Path, value: &Value) -> Result<()> {
    documents::write_replace(path, value)
}

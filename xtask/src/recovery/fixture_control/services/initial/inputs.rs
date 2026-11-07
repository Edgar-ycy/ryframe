//! 所有入口复用已发布 bootstrap；保持描述符原路径拼写并核验实际文件身份。
use super::*;
use crate::recovery::fixture_control::services::lifecycle::query;

pub(super) struct Inputs {
    pub(super) backend: PathBuf,
    pub(super) run: PathBuf,
    pub(super) review: Value,
    pub(super) bootstrap: Value,
    pub(super) private: Value,
    pub(super) sources: Value,
    pub(super) configuration: Value,
    snapshots: Vec<Value>,
}

impl Inputs {
    pub(super) fn load(backend: &Path, review: &Path, environment: &Path) -> Result<Self> {
        let (review_file, review) = read_document(backend, review)?;
        crate::recovery::fixture_control::review::validate_review(&review)?;
        let prepared = query::dispatch(
            backend,
            "service-prepared",
            &json!({"path":environment,
            "secret_files":["mysql-client.cnf","rustfs-access-key.txt","rustfs-secret-key.txt",
            "redis-password.txt","reset-admin-password.txt","jwt-secret.txt","metrics-token.txt"]}),
        )?;
        let bootstrap_file = PathBuf::from(document::text(&prepared[0], "bootstrap 文件")?);
        let bootstrap = prepared[3].clone();
        query::dispatch(
            backend,
            "service-environment",
            &json!({"review_file":review_file,
            "review":review,"environment":bootstrap_file}),
        )?;
        let run = PathBuf::from(document::text(
            &query::dispatch(backend, "service-run", &json!({"review":review}))?,
            "服务根目录",
        )?);
        let environment_file = bootstrap_file
            .parent()
            .ok_or("bootstrap 无父目录")?
            .join("environment.json");
        let sources = json!({"review":common::binding(&review_file)?,"bootstrap":common::binding(&bootstrap_file)?,
            "environment":common::binding(&environment_file)?});
        let mut paths = vec![review_file, bootstrap_file, environment_file];
        for descriptor in bootstrap["secret_files"]
            .as_object()
            .ok_or("bootstrap 凭据清单无效")?
            .values()
        {
            paths.push(common::bound_file(
                backend,
                descriptor,
                "bootstrap 原始凭据",
            )?);
        }
        let execution = Path::new(document::text(&bootstrap["execution_backend"], "执行源码")?);
        let configuration =
            crate::recovery::clone::native::s3::app_configuration::configuration_value(
                execution,
                Path::new(document::text(&bootstrap["config_dir"], "配置目录")?),
                "test",
            )?;
        for name in ["app.toml", "app.test.toml"] {
            let path =
                Path::new(document::text(&configuration["directory"], "配置目录")?).join(name);
            if path.try_exists()? {
                paths.push(path);
            }
        }
        let snapshots = paths
            .iter()
            .map(|path| storage::binding::snapshot(path))
            .collect::<Result<Vec<_>>>()?;
        let inputs = Self {
            backend: backend.into(),
            run,
            review,
            bootstrap,
            private: prepared[2].clone(),
            sources,
            configuration,
            snapshots,
        };
        inputs.guard()?;
        Ok(inputs)
    }

    pub(super) fn guard(&self) -> Result<()> {
        for snapshot in &self.snapshots {
            storage::binding::verify_snapshot(snapshot)?;
        }
        check(
            crate::recovery::fixture_control::artifact::runtime::configuration_digest(Path::new(
                document::text(&self.bootstrap["config_dir"], "原始配置目录")?,
            ))? == document::text(&self.bootstrap["configuration_sha256"], "原始配置摘要")?,
            "首代服务应用配置不同于原始 bootstrap",
        )?;
        let (code, response) = crate::recovery::fixture_control::environment::protocol::execute(
            &serde_json::to_vec(&json!({"format_version":1,"operation":"preflight-binding",
                "arguments":self.review,"write":false}))?,
        );
        check(code == 0 && response["ok"] == true, "首代服务预检绑定无效")?;
        check(
            crate::recovery::clone::native::cache::review_tools::observe(
                &self.backend,
                &self.review,
            )? == response["result"],
            "首代服务工具不同于原审阅",
        )?;
        for snapshot in &self.snapshots {
            storage::binding::verify_snapshot(snapshot)?;
        }
        Ok(())
    }

    pub(super) fn settings(&self) -> Result<Value> {
        let execution = Path::new(document::text(
            &self.bootstrap["execution_backend"],
            "执行源码",
        )?);
        let table = crate::recovery::clone::native::s3::app_configuration::load(
            execution,
            &self.configuration,
            &mut Vec::new(),
        )?;
        serde_json::to_value(table).map_err(Into::into)
    }
}

fn read_document(backend: &Path, path: &Path) -> Result<(PathBuf, Value)> {
    let value = query::dispatch(backend, "service-document", &json!({"path":path}))?;
    Ok((
        PathBuf::from(document::text(&value[0], "输入文件")?),
        value[1].clone(),
    ))
}

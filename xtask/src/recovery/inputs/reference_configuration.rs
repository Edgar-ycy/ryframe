use super::{
    arm::Arm,
    document::{self, Document},
};
use crate::Result;
use crate::recovery::clone::native::{export, s3, target_verify::input::request_configuration};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

pub(super) struct SourceProjection {
    pub(super) source: Value,
    pub(super) tools: Value,
    runtime: Document,
    files: BTreeMap<String, Value>,
}

impl SourceProjection {
    pub(super) fn load(backend: &Path, request: &Document) -> Result<Self> {
        let (_, runtime) = document::bound_file(backend, &request.value["runtime"], "源运行收据")?;
        let root = export::input::execution_backend(backend, &request.value)?;
        let environment = BTreeMap::from([("APP_ENV".into(), "test".into())]);
        let (table, files) = s3::app_configuration::runtime_configuration(
            &root,
            &environment,
            &runtime.value,
            "api",
        )?;
        let directory = runtime.value["api_config_dir"]
            .as_str()
            .ok_or("源运行收据缺少 API 配置目录")?;
        let mut source = request.value["source"].clone();
        source["s3"] = project(&root, Path::new(directory), &table, &source["s3"])?;
        let tools = reference_tools(&request.value)?;
        runtime.assert_unchanged()?;
        request.assert_unchanged()?;
        Ok(Self {
            source,
            tools,
            runtime,
            files,
        })
    }

    pub(super) fn assert_unchanged(&self, backend: &Path, request: &Document) -> Result<()> {
        self.runtime.assert_unchanged()?;
        let current = Self::load(backend, request)?;
        if self.source != current.source
            || self.tools != current.tools
            || self.files != current.files
        {
            return Err("参考源配置在核验期间变化".into());
        }
        request.assert_unchanged()?;
        Ok(())
    }
}

pub(super) fn reference_tools(request: &Value) -> Result<Value> {
    let tools = document::object(
        request.get("tools").ok_or("共享源请求缺少 tools")?,
        "共享源工具",
    )?;
    Ok(json!({
        "mysql": tools.get("mysql").ok_or("共享源请求缺少 mysql")?,
        "mysqldump": tools.get("mysqldump").ok_or("共享源请求缺少 mysqldump")?,
    }))
}

pub(super) struct TargetProjection {
    pub(super) target: Value,
    files: BTreeMap<String, Value>,
}

impl TargetProjection {
    pub(super) fn load(target: &super::target::TargetRequest) -> Result<Self> {
        let environment = BTreeMap::from([("APP_ENV".into(), "test".into())]);
        let request = &target.document.value;
        let (directory, files) =
            request_configuration::fresh_files(&target.execution, &environment, request)?;
        if request["configuration_sha256"].as_str()
            != Some(request_configuration::configuration_identity(&directory, &files)?.as_str())
        {
            return Err("目标 TOML 配置与请求摘要不同".into());
        }
        let table = crate::rate_limit::load_app_table(&target.execution, Some(&directory), "test")
            .map_err(|_| "目标 TOML 配置无法加载；诊断不回显配置内容")?;
        let mut projected = request["target"].clone();
        projected["s3"] = project(
            &target.execution,
            &directory,
            &serde_json::to_value(table)?,
            &request["target"]["s3"],
        )?;
        if request_configuration::fresh_files(&target.execution, &environment, request)?
            != (directory, files.clone())
        {
            return Err("目标配置在读取期间变化".into());
        }
        target.document.assert_unchanged()?;
        Ok(Self {
            target: projected,
            files,
        })
    }

    pub(super) fn assert_unchanged(&self, request: &super::target::TargetRequest) -> Result<()> {
        let current = Self::load(request)?;
        if self.target != current.target || self.files != current.files {
            return Err("参考目标配置在核验期间变化".into());
        }
        Ok(())
    }
}

pub(super) struct Projection {
    pub(super) source: SourceProjection,
    pub(super) target: TargetProjection,
}

impl Projection {
    pub(super) fn load(backend: &Path, arm: &Arm) -> Result<Self> {
        let source = SourceProjection::load(backend, &arm.source_request)?;
        let target = TargetProjection::load(&arm.target)?;
        source.assert_unchanged(backend, &arm.source_request)?;
        target.assert_unchanged(&arm.target)?;
        Ok(Self { source, target })
    }

    pub(super) fn assert_unchanged(&self, backend: &Path, arm: &Arm) -> Result<()> {
        self.source.assert_unchanged(backend, &arm.source_request)?;
        self.target.assert_unchanged(&arm.target)
    }
}

fn project(root: &Path, directory: &Path, table: &Value, private: &Value) -> Result<Value> {
    for field in ["endpoint", "region", "access_key", "secret_key"] {
        let actual = table["object_storage"][field]
            .as_str()
            .filter(|value| !value.is_empty());
        if actual.is_none() || actual != private[field].as_str() {
            let reason = if actual.is_none() {
                "缺失"
            } else {
                "与绑定私有请求不一致"
            };
            return Err(format!(
                "配置文件 {} 的 object_storage.{field}{reason}；请核对文件与私有请求并重新审阅来源；当前只读计划未写入，不自动重放",
                directory.join("app.test.toml").display(),
            ).into());
        }
    }
    let value = json!({"endpoint":private["endpoint"], "region":private["region"],
        "configuration":s3::app_configuration::configuration_value(root, directory, "test")?});
    s3::Configuration::read_app_binding(root, &value)?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_projection_requires_exact_private_fields_and_runtime_identity() {
        let root = crate::workspace::root_dir();
        let holder = tempfile::Builder::new()
            .prefix("reference-configuration-")
            .tempdir_in(root.join(".local-tests"))
            .unwrap();
        let directory = holder.path();
        let private = json!({"endpoint":"http://127.0.0.1:19000","region":"us-east-1","access_key":"private-access","secret_key":"private-secret"});
        let table = json!({"object_storage":private});
        std::fs::write(directory.join("app.test.toml"), "[object_storage]\nendpoint='http://127.0.0.1:19000'\nregion='us-east-1'\naccess_key='private-access'\nsecret_key='private-secret'\n").unwrap();
        let descriptor = project(&root, &directory, &table, &private).unwrap();
        assert_eq!(
            descriptor,
            json!({"endpoint":private["endpoint"],"region":private["region"],"configuration":s3::app_configuration::configuration_value(&root, &directory, "test").unwrap()})
        );
        for field in ["endpoint", "region", "access_key", "secret_key"] {
            let mut different = private.clone();
            different[field] = json!("mismatch-private-value");
            let mut missing = table.clone();
            missing["object_storage"]
                .as_object_mut()
                .unwrap()
                .remove(field);
            for (configuration, request, reason) in [
                (&table, &different, "与绑定私有请求不一致"),
                (&missing, &private, "缺失"),
            ] {
                let error = project(&root, directory, configuration, request)
                    .err()
                    .unwrap()
                    .to_string();
                assert!(error.contains(&directory.join("app.test.toml").display().to_string()));
                assert!(error.contains(&format!("object_storage.{field}{reason}")));
                assert!(error.contains("核对文件与私有请求并重新审阅来源"));
                assert!(error.contains("当前只读计划未写入，不自动重放"));
                for secret in ["private-secret", "private-access", "mismatch-private-value"] {
                    assert!(!error.contains(secret));
                }
            }
        }
        let environment = BTreeMap::from([("APP_ENV".into(), "test".into())]);
        let (selected, files) = request_configuration::configuration_files(
            &root,
            &environment,
            Some(&json!(directory)),
        )
        .unwrap();
        let runtime = json!({"api_config_dir":directory,"api_configuration_sha256":request_configuration::configuration_identity(&selected, &files).unwrap()});
        s3::app_configuration::runtime_configuration(&root, &environment, &runtime, "api").unwrap();
        for field in ["api_config_dir", "api_configuration_sha256"] {
            let mut missing = runtime.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(
                s3::app_configuration::runtime_configuration(&root, &environment, &missing, "api")
                    .is_err()
            );
        }
        let mut wrong = runtime;
        wrong["api_config_dir"] = json!(root.join(".local-tests"));
        assert!(
            s3::app_configuration::runtime_configuration(&root, &environment, &wrong, "api")
                .is_err()
        );
    }
}

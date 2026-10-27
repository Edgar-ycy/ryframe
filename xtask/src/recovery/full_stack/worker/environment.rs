//! 私有环境只从冻结文件取得；公开收据不保存环境值。
use super::operation;
use crate::{
    Result,
    recovery::clone::native::{json, resources::common, target_verify::guard::Inputs},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, process::Command};

pub(super) struct Environment {
    guard: Inputs,
    binding: Value,
    values: BTreeMap<String, String>,
}

impl Environment {
    pub fn operation(request: &Value) -> Result<Option<Self>> {
        request
            .get("environment_binding")
            .map(Self::recorded)
            .transpose()
    }

    pub fn arguments(args: &Value, directory: &Path) -> Result<Option<Self>> {
        let role = super::role::Role::parse(&args["role"])?;
        let op = operation::Operation::read(role, directory, &operation::source(directory)?)?;
        if op.request["backend_root"] != args["backend"]
            || op.request["operation_id"] != args["operation_id"]
        {
            return Err("监督环境不属于当前角色操作".into());
        }
        Self::operation(&op.request)
    }

    pub fn capture(file: &Value) -> Result<Self> {
        crate::recovery::clone::native::lifecycle::manifest::validate_binding_shape(
            file,
            "启动环境文件绑定",
        )?;
        let path = Path::new(file["path"].as_str().ok_or("启动环境缺少路径")?);
        crate::local_test_path::validate_absolute_path(
            path,
            crate::local_test_path::LocalTestPathKind::ExistingFile,
        )?;
        if path.file_name().and_then(|v| v.to_str()) != Some("environment.json") {
            return Err("启动环境必须是已绑定的environment.json".into());
        }
        let guard = Inputs::open([path.to_path_buf()])?;
        let metadata = std::fs::symlink_metadata(path)?;
        if operation::file(path)? != json!({"bytes":file["bytes"],"sha256":file["sha256"]})
            || common::link_count_for(path, &metadata)? != 1
        {
            return Err("启动环境文件与原绑定不同".into());
        }
        let value = operation::read(path)?;
        json::exact(&value, &["environment"], "启动环境文件")?;
        let values = value["environment"]
            .as_object()
            .ok_or("启动环境必须是对象")?;
        if values.get("APP_ENV").and_then(Value::as_str) != Some("test")
            || values.iter().any(|(key, value)| {
                !matches!(key.as_str(), "APP_ENV" | "TEMP" | "TMP")
                    || !value
                        .as_str()
                        .is_some_and(|v| !v.is_empty() && !v.contains('\0'))
            })
        {
            return Err("启动环境只接受APP_ENV=test及非空TEMP/TMP字符串".into());
        }
        let values = values
            .iter()
            .map(|(key, value)| {
                Ok((
                    key.clone(),
                    value.as_str().ok_or("启动环境值无效")?.to_owned(),
                ))
            })
            .collect::<Result<_>>()?;
        let binding = json!({"file":file,"identity":common::device_inode_for(path,&metadata)?});
        let environment = Self {
            guard,
            binding,
            values,
        };
        environment.check()?;
        Ok(environment)
    }

    pub fn recorded(binding: &Value) -> Result<Self> {
        Self::shape(binding)?;
        let environment = Self::capture(&binding["file"])?;
        if environment.binding != *binding {
            return Err("启动环境文件身份已变化；保留操作，不重放".into());
        }
        Ok(environment)
    }

    pub fn shape(binding: &Value) -> Result<()> {
        json::exact(binding, &["file", "identity"], "启动环境身份绑定")?;
        crate::recovery::clone::native::lifecycle::manifest::validate_binding_shape(
            &binding["file"],
            "启动环境",
        )?;
        if !binding["identity"]
            .as_array()
            .is_some_and(|v| v.len() == 2 && v.iter().all(Value::is_u64))
        {
            return Err("启动环境文件身份必须是完整设备号和文件号".into());
        }
        Ok(())
    }

    pub fn binding(&self) -> &Value {
        &self.binding
    }
    pub fn check(&self) -> Result<()> {
        self.guard.check()
    }

    pub fn apply(&self, command: &mut Command) -> Result<()> {
        self.check()?;
        // source不以宿主TEMP/TMP替代冻结环境；未登记的键保持缺失。
        for key in ["APP_ENV", "TEMP", "TMP"] {
            command.env_remove(key);
        }
        command.envs(&self.values);
        self.check()
    }

    #[cfg(windows)]
    pub fn inherited(&self) -> Result<()> {
        self.check()?;
        for key in ["APP_ENV", "TEMP", "TMP"] {
            if std::env::var_os(key) != self.values.get(key).map(std::ffi::OsString::from) {
                return Err("监督跳板实际环境与冻结文件不一致".into());
            }
        }
        self.check()
    }
}

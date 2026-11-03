//! 已准备环境只读核验，凭据固定到普通文件与精确摘要。
use super::*;
use crate::local_test_path::{LocalTestPathKind, validate_local_test_path};
use crate::recovery::clone::native::{
    digest, lifecycle::manifest, resources::common, target_verify::guard,
};
use crate::recovery::fixture_control::environment::plan::CheckedPlan;

struct Window {
    backend: PathBuf,
    receipt_path: PathBuf,
    receipt: Value,
    execution: PathBuf,
    private: Option<Value>,
    names: Vec<String>,
    plan: CheckedPlan,
    files: Files,
}

struct Files {
    protection: guard::Inputs,
    configuration: PathBuf,
    configuration_identity: (u64, u64),
    secrets: Vec<PathBuf>,
}

pub(super) fn load(backend: &Path, selected: &Path, names: &Value) -> Result<Value> {
    let window = Window::open(backend, selected, names)?;
    window.verify()?;
    let private = window.private.ok_or("prepared 读取窗口缺少私有环境")?;
    Ok(json!([
        window.receipt_path,
        window.execution,
        private,
        window.receipt
    ]))
}

impl Window {
    fn open(backend: &Path, selected: &Path, names: &Value) -> Result<Self> {
        let path = if selected.is_absolute() {
            selected.to_owned()
        } else {
            backend.join(selected)
        };
        validate_local_test_path(&path, backend, LocalTestPathKind::ExistingFile)?;
        let bootstrap = guard::Inputs::open([path])?;
        let result = Self::read(backend, selected, names);
        let after = bootstrap.check();
        match (result, after) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
            (Err(error), Err(after)) => {
                Err(format!("{error}；bootstrap 后验同时失败：{after}").into())
            }
        }
    }

    fn read(backend: &Path, selected: &Path, names: &Value) -> Result<Self> {
        let (receipt_path, receipt) = context::document(backend, selected)?;
        let names = validate(&receipt, names)?;
        let execution = PathBuf::from(text(&receipt, "execution_backend")?);
        validate_local_test_path(&execution, backend, LocalTestPathKind::ExistingDirectory)?;
        check(execution.join("Cargo.toml").is_file(), "夹具执行工作树无效")?;
        check(
            receipt["plan"].get("execution_source").is_some(),
            "prepared 缺少执行源码发布绑定；使用正式 execution-source 准备新环境，不升级或改写旧收据",
        )?;
        let files = Files::open(backend, &receipt_path, &execution, &receipt, &names)?;
        let plan = match CheckedPlan::load(backend, &receipt["plan"]) {
            Ok(plan) => plan,
            Err(error) => {
                return match files.protection.check() {
                    Ok(()) => Err(error),
                    Err(after) => {
                        Err(format!("{error}；prepared 文件后验同时失败：{after}").into())
                    }
                };
            }
        };
        let mut window = Self {
            backend: backend.to_owned(),
            receipt_path,
            receipt,
            execution,
            private: None,
            names,
            plan,
            files,
        };
        let result = (|| {
            check(
                window.receipt["plan"]["execution_backend"] == window.receipt["execution_backend"],
                "已发布环境执行根与计划不一致",
            )?;
            configuration(&window.execution, &window.receipt)?;
            window.private = Some(private_environment(
                &window.backend,
                &window.receipt_path,
                &window.receipt,
            )?);
            check(
                secret_paths(&window.backend, &window.receipt, &window.names)?
                    == window.files.secrets,
                "夹具秘密文件路径在读取期间变化",
            )?;
            window.files.protection.check()
        })();
        if let Err(error) = result {
            return match window.verify() {
                Ok(()) => Err(error),
                Err(after) => Err(format!("{error}；prepared 后验同时失败：{after}").into()),
            };
        }
        Ok(window)
    }

    fn verify(&self) -> Result<()> {
        let before = self.files.protection.check();
        let plan = self.plan.verify();
        let configuration = (|| {
            configuration(&self.execution, &self.receipt)?;
            check(
                common::device_inode_for(
                    &self.files.configuration,
                    &std::fs::symlink_metadata(&self.files.configuration)?,
                )? == self.files.configuration_identity,
                "夹具配置目录身份在读取期间变化",
            )
        })();
        let private = private_environment(&self.backend, &self.receipt_path, &self.receipt)
            .and_then(|value| {
                check(
                    self.private
                        .as_ref()
                        .is_none_or(|expected| value == *expected),
                    "夹具私有环境在读取期间变化",
                )
            });
        let secrets = secret_paths(&self.backend, &self.receipt, &self.names).and_then(|value| {
            check(
                value == self.files.secrets,
                "夹具秘密文件路径在读取期间变化",
            )
        });
        let receipt = context::document(&self.backend, &self.receipt_path)
            .and_then(|value| check(value.1 == self.receipt, "prepared 收据在核验期间变化"));
        let mut result = Ok(());
        for checked in [
            before,
            plan,
            configuration,
            private,
            secrets,
            receipt,
            self.files.protection.check(),
        ] {
            result = match (result, checked) {
                (Ok(()), value) | (value, Ok(())) => value,
                (Err(error), Err(after)) => {
                    Err(format!("{error}；prepared 后验同时失败：{after}").into())
                }
            };
        }
        result
    }
}

impl Files {
    fn open(
        backend: &Path,
        receipt_path: &Path,
        execution: &Path,
        receipt: &Value,
        names: &[String],
    ) -> Result<Self> {
        let configuration = PathBuf::from(text(receipt, "config_dir")?);
        validate_local_test_path(
            &configuration,
            execution,
            LocalTestPathKind::ExistingDirectory,
        )?;
        let configuration_identity =
            common::device_inode_for(&configuration, &std::fs::symlink_metadata(&configuration)?)?;
        let secrets = secret_locations(backend, receipt, names)?;
        let mut paths = vec![
            receipt_path.to_owned(),
            receipt_path
                .parent()
                .ok_or("环境收据没有父目录")?
                .join("environment.json"),
            configuration.join("app.test.toml"),
        ];
        let common = configuration.join("app.toml");
        if common.try_exists()? {
            paths.push(common);
        }
        paths.extend(secrets.iter().cloned());
        let protection =
            guard::Inputs::open(paths.into_iter().collect::<std::collections::BTreeSet<_>>())?;
        Ok(Self {
            protection,
            configuration,
            configuration_identity,
            secrets,
        })
    }
}

fn validate(receipt: &Value, names: &Value) -> Result<Vec<String>> {
    let names = names.as_array().ok_or("秘密文件清单必须为数组")?;
    check(
        receipt["format_version"] == 1
            && receipt["kind"] == "reference-fixture-environment"
            && receipt["status"] == "prepared"
            && receipt["services_started"] == false
            && receipt["remote_writes"] == 0
            && receipt["historical_data_used"] == false
            && receipt["execution_backend"].is_string()
            && receipt["environment_sha256"].is_string(),
        "夹具运行时只能使用尚未启动服务的已准备私有环境",
    )?;
    let secrets = receipt["secret_files"]
        .as_object()
        .ok_or("夹具秘密文件描述不完整")?;
    check(
        names.len() == secrets.len()
            && names
                .iter()
                .all(|name| name.as_str().is_some_and(|name| secrets.contains_key(name))),
        "夹具秘密文件清单与 bootstrap 不同",
    )?;
    names
        .iter()
        .map(|name| {
            name.as_str()
                .map(str::to_owned)
                .ok_or_else(|| "秘密文件名必须为字符串".into())
        })
        .collect()
}

fn secret_paths(backend: &Path, receipt: &Value, names: &[String]) -> Result<Vec<PathBuf>> {
    let secrets = &receipt["secret_files"];
    let mut paths = Vec::new();
    for name in names {
        let descriptor = &secrets[name.as_str()];
        let path = manifest::bound_file(backend, descriptor, "夹具秘密文件", true)
            .map_err(|_| model::error("夹具秘密文件与 bootstrap 绑定不一致"))?;
        check(
            path.file_name().is_some_and(|file| file == name.as_str())
                && binding(&path)? == *descriptor,
            "夹具秘密文件与 bootstrap 绑定不一致",
        )?;
        paths.push(path);
    }
    Ok(paths)
}

fn secret_locations(backend: &Path, receipt: &Value, names: &[String]) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for name in names {
        let descriptor = &receipt["secret_files"][name.as_str()];
        manifest::validate_binding_shape(descriptor, "夹具秘密文件")?;
        let path = PathBuf::from(text(descriptor, "path")?);
        validate_local_test_path(&path, backend, LocalTestPathKind::ExistingFile)?;
        check(
            path.file_name().is_some_and(|file| file == name.as_str()),
            "夹具秘密文件与 bootstrap 绑定不一致",
        )?;
        paths.push(path);
    }
    Ok(paths)
}

fn configuration(execution: &Path, receipt: &Value) -> Result<()> {
    let config = PathBuf::from(text(receipt, "config_dir")?);
    validate_local_test_path(&config, execution, LocalTestPathKind::ExistingDirectory)?;
    check(
        super::super::super::artifact::runtime::configuration_digest(&config)?
            == receipt["configuration_sha256"],
        "夹具配置目录或配置文件与 bootstrap 绑定不一致",
    )?;
    Ok(())
}

fn private_environment(backend: &Path, receipt_path: &Path, receipt: &Value) -> Result<Value> {
    let (_, environment) = context::document(
        backend,
        &receipt_path
            .parent()
            .ok_or("环境收据没有父目录")?
            .join("environment.json"),
    )?;
    json_io::exact(&environment, &["environment"], "夹具私有环境文件")?;
    let private = &environment["environment"];
    let values = private.as_object().ok_or("夹具私有环境文件字段无效")?;
    check(
        values.values().all(Value::is_string)
            && digest::plan_hash(private)? == receipt["environment_sha256"],
        "夹具私有环境与 bootstrap 摘要不一致",
    )?;
    check(
        values
            .keys()
            .all(|key| ["APP_ENV", "TEMP", "TMP"].contains(&key.as_str()))
            && private["APP_ENV"] == "test",
        "夹具进程环境只允许 APP_ENV 与受管临时目录",
    )?;
    Ok(private.clone())
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/fixture_control/services/prepared_window.rs"]
mod tests;

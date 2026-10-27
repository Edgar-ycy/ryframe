//! 构建继承系统运行环境、Cargo 目标目录和 prepared 已冻结的三个值。
use super::super::{model::*, plan, runtime};
use crate::Result;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub(super) struct Inputs {
    pub root: PathBuf,
    pub configuration: PathBuf,
    pub environment: BTreeMap<String, String>,
    guards: Vec<Value>,
}

impl Inputs {
    pub fn load(backend: &Path, environment: &Path) -> Result<Self> {
        let boot = runtime::bootstrap(backend, environment)?;
        let bootstrap = path(&boot["bootstrap"])?;
        let parent = bootstrap.parent().ok_or("夹具环境收据缺少父目录")?;
        let (receipt, guard) = document(&bootstrap)?;
        let (values, environment_guard) = document(&parent.join("environment.json"))?;
        crate::recovery::clone::native::json::exact(&values, &["environment"], "夹具私有环境")?;
        check(
            values["environment"] == boot["private"],
            "夹具环境与 prepared 结果不匹配",
        )?;
        let mut guards = plan::input_guards(&receipt)?;
        guards.extend([guard, environment_guard]);
        for name in ["app.toml", "app.test.toml"] {
            guards.push(snapshot(&parent.join("config").join(name), MAX_BYTES)?);
        }
        check(
            runtime::bootstrap(backend, environment)? == boot,
            "夹具 prepared 输入在冻结期间变化",
        )?;
        let value = Self {
            root: path(&boot["execution"])?,
            configuration: parent.join("config"),
            environment: configured(&boot["private"], std::env::vars_os())?,
            guards,
        };
        value.verify()?;
        Ok(value)
    }

    pub fn verify(&self) -> Result<()> {
        for guard in &self.guards {
            unchanged(guard)?;
        }
        Ok(())
    }
}

pub(super) fn configured(
    private: &Value,
    inherited: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
) -> Result<BTreeMap<String, String>> {
    let private = private.as_object().ok_or("夹具私有环境必须为对象")?;
    check(
        private
            .keys()
            .all(|key| ["APP_ENV", "TEMP", "TMP"].contains(&key.as_str()))
            && private.get("APP_ENV").and_then(Value::as_str) == Some("test"),
        "夹具构建环境必须明确选择 test",
    )?;
    let mut values = BTreeMap::new();
    for (key, value) in inherited {
        let key = key.to_str().ok_or("系统环境名称不是 Unicode")?;
        let upper = key.to_ascii_uppercase();
        if !private.contains_key(&upper) && (system_name(&upper) || upper == "CARGO_TARGET_DIR") {
            values.insert(
                key.to_owned(),
                value.to_str().ok_or("系统环境值不是 Unicode")?.to_owned(),
            );
        }
    }
    for (key, value) in private {
        values.insert(key.clone(), text(value)?.to_owned());
    }
    Ok(values)
}

fn system_name(name: &str) -> bool {
    [
        "APPDATA",
        "COMMONPROGRAMFILES",
        "COMMONPROGRAMFILES(X86)",
        "COMMONPROGRAMW6432",
        "COMSPEC",
        "HOME",
        "HOMEDRIVE",
        "HOMEPATH",
        "LANG",
        "LOCALAPPDATA",
        "NUMBER_OF_PROCESSORS",
        "OS",
        "PATH",
        "PATHEXT",
        "PROGRAMDATA",
        "PROGRAMFILES",
        "PROGRAMFILES(X86)",
        "PROGRAMW6432",
        "SHELL",
        "SYSTEMDRIVE",
        "SYSTEMROOT",
        "TEMP",
        "TMP",
        "TMPDIR",
        "USER",
        "USERPROFILE",
        "WINDIR",
    ]
    .contains(&name)
        || name.starts_with("PROCESSOR_")
        || name.starts_with("LC_")
}

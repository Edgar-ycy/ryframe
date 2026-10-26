//! 来源生产者仅继承操作系统运行环境；项目配置固定选择 APP_ENV=test。
use std::collections::{BTreeMap, BTreeSet};

use crate::Result;

const SYSTEM: &[&str] = &[
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
];

pub(super) fn configured(private: &BTreeMap<String, String>) -> Result<BTreeMap<String, String>> {
    if private.get("APP_ENV").map(String::as_str) != Some("test") {
        return Err("来源业务生产者只能选择 APP_ENV=test；配置和凭据必须来自绑定文件".into());
    }
    let mut names = BTreeSet::new();
    for (name, value) in private {
        let name = name.to_ascii_uppercase();
        if !matches!(name.as_str(), "APP_ENV" | "TEMP" | "TMP")
            || !names.insert(name)
            || value.is_empty()
            || value.contains('\0')
        {
            return Err("来源私有运行环境只允许唯一的 APP_ENV、TEMP、TMP 文本字段；请将应用配置和凭据写入绑定配置文件".into());
        }
    }
    let mut values = std::env::vars()
        .filter(|(name, _)| {
            let name = name.to_ascii_uppercase();
            !names.contains(&name)
                && (SYSTEM.contains(&name.as_str())
                    || name.starts_with("PROCESSOR_")
                    || name.starts_with("LC_"))
        })
        .collect::<BTreeMap<_, _>>();
    values.extend(private.clone());
    Ok(values)
}

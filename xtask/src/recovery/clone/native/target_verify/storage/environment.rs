use std::collections::BTreeMap;

use super::StorageInputs;

impl StorageInputs<'_> {
    pub(crate) fn command_environment(&self) -> BTreeMap<String, String> {
        let mut environment = std::env::vars()
            .filter(|(name, _)| {
                let key = name.to_ascii_uppercase();
                SYSTEM.contains(&key.as_str())
                    || key.starts_with("PROCESSOR_")
                    || key.starts_with("LC_")
            })
            .filter(|(key, _)| {
                !self
                    .environment
                    .keys()
                    .any(|private| private.eq_ignore_ascii_case(key))
            })
            .collect::<BTreeMap<_, _>>();
        environment.extend(self.environment.clone());
        environment
    }
}

// 与既有 configured 私有环境合并规则一致，只继承操作系统运行字段。
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

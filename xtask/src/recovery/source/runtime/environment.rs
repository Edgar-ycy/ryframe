//! 来源生产者仅继承操作系统运行环境；项目配置固定选择 APP_ENV=test。
use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

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

pub(in crate::recovery) fn validate_private(
    private: &BTreeMap<String, String>,
) -> Result<BTreeSet<String>> {
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
    Ok(names)
}

pub(super) fn configured(private: &BTreeMap<String, String>) -> Result<BTreeMap<String, String>> {
    let names = validate_private(private)?;
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

const COPY_API_ADVICE: &str = "应用仅允许 APP_ENV=test，已冻结的 OS TEMP/TMP 必须原样保留，不能新增、删除或改值；其他配置和凭据使用绑定 TOML，不回显私有值";

pub(super) fn copy_api(args: &Value) -> Result<Value> {
    super::documents::exact(args, &["initial", "api"], "复制后 API 环境参数")
        .map_err(|_| api_diagnostic("arguments", "参数必须仅含 initial 和 api"))?;
    let initial: BTreeMap<String, String> = serde_json::from_value(args["initial"].clone())
        .map_err(|_| api_diagnostic("initial_map", "初始环境必须是文本键值映射"))?;
    let api: BTreeMap<String, String> = serde_json::from_value(args["api"].clone())
        .map_err(|_| api_diagnostic("api_map", "目标 API 环境必须是文本键值映射"))?;
    validate_private(&initial).map_err(|_| {
        api_diagnostic(
            "initial_private",
            "初始环境未通过既有 APP_ENV/TEMP/TMP 私有字段规则",
        )
    })?;
    validate_private(&api).map_err(|_| {
        api_diagnostic(
            "api_private",
            "目标 API 环境未通过既有 APP_ENV/TEMP/TMP 私有字段规则",
        )
    })?;
    if initial != api {
        return Err(api_diagnostic(
            "frozen_context_changed",
            "初始与目标 API 环境映射不完全一致",
        )
        .into());
    }
    Ok(Value::Null)
}

fn api_diagnostic(stage: &'static str, reason: &'static str) -> String {
    format!("复制后 API 环境绑定无效：stage={stage}；原因={reason}。{COPY_API_ADVICE}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rejected(initial: Value, api: Value, stage: &str) {
        let error = copy_api(&json!({"initial":initial,"api":api}))
            .unwrap_err()
            .to_string();
        assert!(error.contains(&format!("stage={stage}；")), "{error}");
        assert!(error.ends_with(COPY_API_ADVICE));
        assert!(!error.contains("private-password-token"));
    }

    #[test]
    fn identical_application_selector_and_frozen_temporary_fields_are_accepted() {
        for value in [
            json!({"APP_ENV":"test"}),
            json!({"APP_ENV":"test","TEMP":"D:/冻结 临时","TMP":"D:/冻结 临时"}),
            json!({"APP_ENV":"test","TEMP":"D:/frozen"}),
        ] {
            assert_eq!(
                copy_api(&json!({"initial":value,"api":value})).unwrap(),
                Value::Null
            );
        }
    }

    #[test]
    fn adding_removing_changing_or_recasing_frozen_temporary_fields_is_rejected() {
        let initial = json!({"APP_ENV":"test","TEMP":"D:/frozen","TMP":"D:/frozen"});
        for api in [
            json!({"APP_ENV":"test"}),
            json!({"APP_ENV":"test","TEMP":"D:/changed","TMP":"D:/frozen"}),
            json!({"APP_ENV":"test","TEMP":"D:/frozen"}),
            json!({"APP_ENV":"test","temp":"D:/frozen","TMP":"D:/frozen"}),
        ] {
            rejected(initial.clone(), api.clone(), "frozen_context_changed");
            rejected(api, initial.clone(), "frozen_context_changed");
        }
    }

    #[test]
    fn invalid_private_maps_and_types_never_disclose_values() {
        for (value, category) in [
            (json!(null), "map"),
            (json!([]), "map"),
            (json!({}), "private"),
            (json!({"APP_ENV":"prod"}), "private"),
            (json!({"app_env":"test"}), "private"),
            (json!({"APP_ENV":false}), "map"),
            (json!({"APP_ENV":"test","TEMP":null}), "map"),
            (json!({"APP_ENV":"test","TEMP":17}), "map"),
            (json!({"APP_ENV":"test","TEMP":""}), "private"),
            (
                json!({"APP_ENV":"test","TEMP":"private-password-token\u{0000}"}),
                "private",
            ),
            (
                json!({"APP_ENV":"test","TEMP":"one","temp":"two"}),
                "private",
            ),
            (
                json!({"APP_ENV":"test","APP_DATABASE_PASSWORD":"private-password-token"}),
                "private",
            ),
            (
                json!({"APP_ENV":"test","HTTP_PROXY":"private-password-token"}),
                "private",
            ),
            (
                json!({"APP_ENV":"test","PYTHONPATH":"private-password-token"}),
                "private",
            ),
            (
                json!({"APP_ENV":"test","NODE_OPTIONS":"private-password-token"}),
                "private",
            ),
        ] {
            rejected(
                value.clone(),
                json!({"APP_ENV":"test"}),
                &format!("initial_{category}"),
            );
            rejected(json!({"APP_ENV":"test"}), value, &format!("api_{category}"));
        }
        let error = copy_api(&json!({"initial":{"APP_ENV":"test"},
            "api":{"APP_ENV":"test"},"private-password-token":true}))
        .unwrap_err()
        .to_string();
        assert!(error.contains("stage=arguments；"));
        assert!(!error.contains("private-password-token"));
    }
}

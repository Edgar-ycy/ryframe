//! Required 输入只作为数据读取，不经环境或脚本解释。
use std::{collections::BTreeMap, fs::File, io::Read, path::Path};

use serde::{Deserialize, Deserializer, de::Error};

use super::{CliError, RequiredAction, RequiredEvent, RequiredNeedDocument, RequiredOptions};

const LIMIT: u64 = 1024 * 1024;
const INVALID: &str = "Required 输入无效：请提供不超过 1 MiB 的 UTF-8 JSON，严格使用 event/action/needs，禁止重复键、未知字段和非法类型";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    event: String,
    #[serde(deserialize_with = "Option::<String>::deserialize")]
    action: Option<String>,
    #[serde(deserialize_with = "unique_map")]
    needs: BTreeMap<String, RequiredNeedDocument>,
}

pub(super) fn read(input: &str) -> Result<RequiredOptions, CliError> {
    if !Path::new(input).is_absolute() {
        return Err(CliError::new("--input 必须是绝对 JSON 文件路径"));
    }
    let mut bytes = Vec::new();
    File::open(input)
        .and_then(|file| file.take(LIMIT + 1).read_to_end(&mut bytes))
        .map_err(|error| {
            CliError::new(format!(
                "Required 输入文件读取失败（类别 {:?}，系统错误码 {:?}）；请核对 --input 文件路径、文件类型及读取权限。未执行汇总。",
                error.kind(), error.raw_os_error()
            ))
        })?;
    if bytes.is_empty() || bytes.len() as u64 > LIMIT {
        return Err(CliError::new(INVALID));
    }
    let document: Document = serde_json::from_slice(&bytes).map_err(|_| CliError::new(INVALID))?;
    let event = RequiredEvent::parse(&document.event)?;
    let action = RequiredAction::parse(event, document.action.as_deref())?;
    // 与直接参数共用原校验和 typed 汇总输入，不改变门禁规则。
    let needs = super::validated_needs(document.needs).map_err(|_| CliError::new(INVALID))?;
    Ok(RequiredOptions {
        event,
        action,
        needs,
    })
}

pub(super) fn needs(value: &str) -> Result<BTreeMap<String, RequiredNeedDocument>, CliError> {
    let mut deserializer = serde_json::Deserializer::from_str(value);
    let result = unique_map(&mut deserializer).map_err(|_| CliError::new(INVALID))?;
    deserializer.end().map_err(|_| CliError::new(INVALID))?;
    Ok(result)
}

pub(super) fn unique_map<'de, D, T>(deserializer: D) -> Result<BTreeMap<String, T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Visitor<T>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for Visitor<T> {
        type Value = BTreeMap<String, T>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("不含重复键的对象")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            mut map: M,
        ) -> Result<Self::Value, M::Error> {
            let mut result = BTreeMap::new();
            while let Some(key) = map.next_key::<String>()? {
                if result.contains_key(&key) {
                    return Err(M::Error::custom("重复键"));
                }
                result.insert(key, map.next_value()?);
            }
            Ok(result)
        }
    }
    deserializer.deserialize_map(Visitor(std::marker::PhantomData))
}

//! 按既有写入器的键序、Unicode 与换行方式重算历史前像，不接受语义近似。
use std::{fmt, path::Path};

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::evidence::{binding, check, exact, text};
use crate::{Result, recovery::clone::native::lifecycle::state};

#[derive(Clone)]
enum Ordered {
    Scalar(Value),
    Array(Vec<Self>),
    Object(Vec<(String, Self)>),
}

impl<'de> Deserialize<'de> for Ordered {
    fn deserialize<D: Deserializer<'de>>(input: D) -> std::result::Result<Self, D::Error> {
        struct OrderedVisitor;
        impl<'de> Visitor<'de> for OrderedVisitor {
            type Value = Ordered;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("JSON 值")
            }
            fn visit_bool<E: de::Error>(self, value: bool) -> std::result::Result<Ordered, E> {
                Ok(Ordered::Scalar(value.into()))
            }
            fn visit_i64<E: de::Error>(self, value: i64) -> std::result::Result<Ordered, E> {
                Ok(Ordered::Scalar(value.into()))
            }
            fn visit_u64<E: de::Error>(self, value: u64) -> std::result::Result<Ordered, E> {
                Ok(Ordered::Scalar(value.into()))
            }
            fn visit_f64<E: de::Error>(self, value: f64) -> std::result::Result<Ordered, E> {
                serde_json::Number::from_f64(value)
                    .map(|item| Ordered::Scalar(Value::Number(item)))
                    .ok_or_else(|| E::custom("非有限数字"))
            }
            fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<Ordered, E> {
                Ok(Ordered::Scalar(value.into()))
            }
            fn visit_none<E: de::Error>(self) -> std::result::Result<Ordered, E> {
                Ok(Ordered::Scalar(Value::Null))
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<Ordered, E> {
                self.visit_none()
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut input: A,
            ) -> std::result::Result<Ordered, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = input.next_element()? {
                    values.push(value);
                }
                Ok(Ordered::Array(values))
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut input: A,
            ) -> std::result::Result<Ordered, A::Error> {
                let mut values = Vec::new();
                while let Some((key, value)) = input.next_entry::<String, Ordered>()? {
                    if values.iter().any(|(existing, _)| existing == &key) {
                        return Err(de::Error::custom("JSON 字段重复"));
                    }
                    values.push((key, value));
                }
                Ok(Ordered::Object(values))
            }
        }
        input.deserialize_any(OrderedVisitor)
    }
}

pub(super) fn count(run: &Path, descriptor: &Value) -> Result<usize> {
    exact(descriptor, &["path", "bytes", "sha256"])?;
    check(
        super::evidence::path_is(&descriptor["path"], &run.join("state.json"))
            && descriptor["bytes"].as_u64().is_some_and(|bytes| bytes > 0),
        "历史阶段记录描述不属于固定运行目录",
    )?;
    let expected = text(descriptor, "sha256")?;
    check(
        expected.len() == 64
            && expected
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "历史阶段记录摘要无效",
    )?;
    let before = binding(&run.join("state.json"))?;
    let state = state::load(run, true)?;
    if &before == descriptor {
        return Ok(state::attempts(&state)?.len());
    }
    let raw = std::fs::read(run.join("state.json"))?;
    let ordered: Ordered = serde_json::from_slice(&raw)?;
    let mut matches = Vec::new();
    for count in 0..=state::attempts(&state)?.len() {
        let mut candidate = ordered.clone();
        let Ordered::Object(fields) = &mut candidate else {
            return Err("历史状态必须为对象".into());
        };
        let Some((_, Ordered::Array(attempts))) =
            fields.iter_mut().find(|(key, _)| key == "attempts")
        else {
            return Err("历史状态缺少阶段列表".into());
        };
        attempts.truncate(count);
        let found = [
            (true, false, false),
            (false, true, false),
            (false, true, true),
        ]
        .into_iter()
        .any(|(sort, ascii, crlf)| {
            let mut encoded = encode(&candidate, 0, sort, ascii);
            encoded.push('\n');
            if crlf {
                encoded = encoded.replace('\n', "\r\n");
            }
            Some(encoded.len() as u64) == descriptor["bytes"].as_u64()
                && hex::encode(Sha256::digest(encoded.as_bytes())) == expected
        });
        if found {
            matches.push(count);
        }
    }
    check(matches.len() == 1, "历史阶段记录不是当前账本的精确追加前缀")?;
    check(
        binding(&run.join("state.json"))? == before,
        "阶段记录在历史前缀核对期间变化",
    )?;
    Ok(matches[0])
}

fn quoted(value: &str, ascii: bool) -> String {
    let encoded = serde_json::to_string(value).expect("字符串序列化不会失败");
    if !ascii {
        return encoded;
    }
    let mut output = String::new();
    for character in encoded.chars() {
        if character < '\u{7f}' {
            output.push(character);
        } else {
            for unit in character.encode_utf16(&mut [0; 2]).iter() {
                output.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    output
}

fn encode(value: &Ordered, depth: usize, sort: bool, ascii: bool) -> String {
    let indent = "  ".repeat(depth + 1);
    let end = "  ".repeat(depth);
    match value {
        Ordered::Scalar(Value::String(value)) => quoted(value, ascii),
        Ordered::Scalar(value) => value.to_string(),
        Ordered::Array(values) => {
            if values.is_empty() {
                return "[]".into();
            }
            format!(
                "[\n{indent}{}\n{end}]",
                values
                    .iter()
                    .map(|value| encode(value, depth + 1, sort, ascii))
                    .collect::<Vec<_>>()
                    .join(&format!(",\n{indent}"))
            )
        }
        Ordered::Object(values) => {
            if values.is_empty() {
                return "{}".into();
            }
            let mut entries = values.iter().collect::<Vec<_>>();
            if sort {
                entries.sort_by(|left, right| left.0.cmp(&right.0));
            }
            format!(
                "{{\n{indent}{}\n{end}}}",
                entries
                    .into_iter()
                    .map(|(key, value)| format!(
                        "{}: {}",
                        quoted(key, ascii),
                        encode(value, depth + 1, sort, ascii)
                    ))
                    .collect::<Vec<_>>()
                    .join(&format!(",\n{indent}"))
            )
        }
    }
}

#[cfg(test)]
pub(super) fn serializations(value: &str) -> Vec<Vec<u8>> {
    let node: Ordered = serde_json::from_str(value).unwrap();
    [
        (true, false, false),
        (false, true, false),
        (false, true, true),
    ]
    .into_iter()
    .map(|(sort, ascii, crlf)| {
        let output = encode(&node, 0, sort, ascii) + "\n";
        if crlf {
            output.replace('\n', "\r\n").into_bytes()
        } else {
            output.into_bytes()
        }
    })
    .collect()
}

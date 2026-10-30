//! 共用账本前缀的有序编码；只匹配既有写入器的完整字节和SHA256，不访问文件。
use crate::Result;
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fmt;

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

/// 只接受唯一前缀的原字节长度与摘要，不以JSON语义相等替代写入器绑定。
pub(crate) fn count(raw: &[u8], bytes: u64, expected_sha: &str) -> Result<usize> {
    let value: Value = serde_json::from_slice(raw)?;
    let ordered = update_order(serde_json::from_slice(raw)?, &value)?;
    let Ordered::Object(fields) = &ordered else {
        return Err("历史状态必须为对象".into());
    };
    let Some((_, Ordered::Array(attempts))) = fields.iter().find(|(key, _)| key == "attempts")
    else {
        return Err("历史状态缺少阶段列表".into());
    };
    if raw.len() as u64 == bytes && hex::encode(Sha256::digest(raw)) == expected_sha {
        return Ok(attempts.len());
    }
    let mut matches = Vec::new();
    for count in 0..=attempts.len() {
        let mut candidate = ordered.clone();
        let Ordered::Object(fields) = &mut candidate else {
            unreachable!()
        };
        let Some((_, Ordered::Array(items))) = fields.iter_mut().find(|(key, _)| key == "attempts")
        else {
            unreachable!()
        };
        items.truncate(count);
        let found = [
            (true, false, false),
            (false, true, false),
            (false, true, true),
            (true, false, true),
        ]
        .into_iter()
        .any(|(sort, ascii, crlf)| {
            let mut encoded = encode(&candidate, 0, sort, ascii) + "\n";
            if crlf {
                encoded = encoded.replace('\n', "\r\n");
            }
            encoded.len() as u64 == bytes
                && hex::encode(Sha256::digest(encoded.as_bytes())) == expected_sha
        });
        if found {
            matches.push(count);
        }
    }
    if matches.len() != 1 {
        return Err("历史阶段记录不是当前账本的精确追加前缀".into());
    }
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
pub(crate) fn serializations(value: &str) -> Vec<Vec<u8>> {
    let semantic: Value = serde_json::from_str(value).unwrap();
    let node = update_order(serde_json::from_str(value).unwrap(), &semantic).unwrap();
    [
        (true, false, false),
        (false, true, false),
        (false, true, true),
        (true, false, true),
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

/// 保留现存账本的字段顺序；新字段与新阶段使用确定性顺序，不改写旧前像语义。
pub(crate) fn updated_serialization(previous: &[u8], next: &Value) -> Result<Vec<u8>> {
    let previous: Ordered = serde_json::from_slice(previous)?;
    let value = update_order(previous, next)?;
    Ok((encode(&value, 0, false, true) + "\n").into_bytes())
}

fn update_order(previous: Ordered, next: &Value) -> Result<Ordered> {
    match (previous, next) {
        (Ordered::Object(fields), Value::Object(values)) => {
            let mut output = Vec::new();
            for (key, old) in fields {
                if let Some(value) = values.get(&key) {
                    output.push((key, update_order(old, value)?));
                }
            }
            for (key, value) in values {
                if !output.iter().any(|(present, _)| present == key) {
                    output.push((key.clone(), from_value(value)));
                }
            }
            Ok(Ordered::Object(output))
        }
        (Ordered::Array(items), Value::Array(values)) => {
            let mut previous = items.into_iter();
            let output = values
                .iter()
                .map(|value| match previous.next() {
                    Some(old) => update_order(old, value),
                    None => Ok(from_value(value)),
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(Ordered::Array(output))
        }
        (_, value) => Ok(from_value(value)),
    }
}

/// 数值直接保留serde_json的真实表示，避免arbitrary_precision的内部map被误作对象。
fn from_value(value: &Value) -> Ordered {
    match value {
        Value::Object(values) => Ordered::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), from_value(value)))
                .collect(),
        ),
        Value::Array(values) => Ordered::Array(values.iter().map(from_value).collect()),
        value => Ordered::Scalar(value.clone()),
    }
}

#[cfg(test)]
#[test]
fn append_preserves_each_existing_writer_prefix_including_unicode() {
    let original = r#"{"format_version":1,"manifest":{"z":"中文","a":1},"attempts":[{"number":1,"sources":{"z":"路径","a":2}}]}"#;
    for previous in serializations(original) {
        let mut next: Value = serde_json::from_slice(&previous).unwrap();
        next["attempts"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"number":2,"sources":{}}));
        let encoded = updated_serialization(&previous, &next).unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&encoded).unwrap(), next);
        let mut prefix: Ordered = serde_json::from_slice(&encoded).unwrap();
        let Ordered::Object(fields) = &mut prefix else {
            unreachable!()
        };
        let (_, Ordered::Array(items)) = fields
            .iter_mut()
            .find(|(key, _)| key == "attempts")
            .unwrap()
        else {
            unreachable!()
        };
        items.truncate(1);
        assert!(serializations(&(encode(&prefix, 0, false, true) + "\n")).contains(&previous));
    }
}

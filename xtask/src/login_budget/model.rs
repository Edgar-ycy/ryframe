use std::{collections::BTreeMap, fmt};

use serde::{
    Deserialize, Deserializer, Serialize,
    de::{self, MapAccess, Visitor},
};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::BudgetError;

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const MAX_BUCKETS: usize = 1024;
const MARGIN_MS: u64 = 250;

/// 字段声明顺序就是 Node JSON.stringify 用于绑定比较的顺序。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Binding {
    pub(crate) scope: String,
    pub(crate) capacity: u64,
    #[serde(rename = "windowMs")]
    pub(crate) window_ms: u64,
}

impl Binding {
    pub(crate) fn new(scope: &str, capacity: u64, window_ms: u64) -> Result<Self, BudgetError> {
        let edge = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
        if !(2..=48).contains(&scope.len())
            || !scope.as_bytes().first().copied().is_some_and(edge)
            || !scope.as_bytes().last().copied().is_some_and(edge)
            || !scope
                .bytes()
                .all(|byte| edge(byte) || matches!(byte, b'_' | b'-'))
        {
            return Err(BudgetError::Scope);
        }
        if !(1..=10_000).contains(&capacity) {
            return Err(BudgetError::Capacity);
        }
        if !(1..=86_400_000).contains(&window_ms) {
            return Err(BudgetError::Window);
        }
        Ok(Self {
            scope: scope.into(),
            capacity,
            window_ms,
        })
    }
}

impl<'de> Deserialize<'de> for Binding {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Ordered;
        impl<'de> Visitor<'de> for Ordered {
            type Value = Binding;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("有序登录预算绑定")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Binding, A::Error> {
                let mut values = Vec::with_capacity(3);
                for expected in ["scope", "capacity", "windowMs"] {
                    if map.next_key::<String>()?.as_deref() != Some(expected) {
                        return Err(de::Error::custom("登录预算绑定字段顺序无效"));
                    }
                    values.push(map.next_value::<Value>()?);
                }
                if map.next_key::<String>()?.is_some() {
                    return Err(de::Error::custom("登录预算绑定字段无效"));
                }
                let scope = values[0]
                    .as_str()
                    .ok_or_else(|| de::Error::custom("绑定无效"))?;
                let capacity = integer(&values[1]).ok_or_else(|| de::Error::custom("绑定无效"))?;
                let window = integer(&values[2]).ok_or_else(|| de::Error::custom("绑定无效"))?;
                Binding::new(scope, capacity, window).map_err(de::Error::custom)
            }
        }
        deserializer.deserialize_map(Ordered)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Reservation {
    pub(crate) key: String,
    pub(crate) generation: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ReserveOutcome {
    pub(crate) wait_ms: u64,
    pub(crate) reservation: Option<Vec<Reservation>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Ledger {
    #[serde(deserialize_with = "safe_integer")]
    version: u64,
    binding: Binding,
    #[serde(rename = "observedAt", deserialize_with = "safe_integer")]
    observed_at: u64,
    buckets: BTreeMap<String, Bucket>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Bucket {
    generation: String,
    #[serde(deserialize_with = "safe_integer")]
    count: u64,
    #[serde(rename = "reservedAt", deserialize_with = "safe_integer")]
    reserved_at: u64,
    #[serde(rename = "completedAt", deserialize_with = "optional_integer")]
    completed_at: Option<u64>,
}

impl Ledger {
    pub(crate) fn empty(binding: Binding, now_ms: u64) -> Result<Self, BudgetError> {
        validate_time(now_ms)?;
        Ok(Self {
            version: 1,
            binding,
            observed_at: now_ms,
            buckets: BTreeMap::new(),
        })
    }

    pub(crate) fn validate(&self, binding: &Binding, now_ms: u64) -> Result<(), BudgetError> {
        validate_time(now_ms)?;
        if self.version != 1
            || &self.binding != binding
            || self.observed_at > now_ms
            || self.buckets.len() > MAX_BUCKETS
        {
            return Err(BudgetError::Ledger);
        }
        for (key, bucket) in &self.buckets {
            if !valid_key(key)
                || bucket.generation.len() != 36
                || !bucket
                    .generation
                    .bytes()
                    .all(|byte| hex_byte(byte) || byte == b'-')
                || !(1..=binding.capacity).contains(&bucket.count)
                || bucket.reserved_at > now_ms
                || bucket.completed_at.is_some_and(|time| time > now_ms)
            {
                return Err(BudgetError::Bucket);
            }
        }
        Ok(())
    }

    pub(crate) fn reserve(
        &mut self,
        binding: &Binding,
        keys: &[String; 2],
        now_ms: u64,
    ) -> Result<ReserveOutcome, BudgetError> {
        self.validate(binding, now_ms)?;
        self.buckets
            .retain(|_, bucket| expires_at(bucket, binding) > now_ms as f64);
        self.observed_at = now_ms;
        let wait_ms = keys
            .iter()
            .filter_map(|key| self.buckets.get(key))
            .filter(|bucket| bucket.count >= binding.capacity)
            .map(|bucket| (expires_at(bucket, binding) - now_ms as f64) as u64)
            .max()
            .unwrap_or(0);
        if wait_ms > 0 {
            return Ok(ReserveOutcome {
                wait_ms,
                reservation: None,
            });
        }
        let missing = keys
            .iter()
            .filter(|key| !self.buckets.contains_key(*key))
            .count();
        if self.buckets.len() + missing > MAX_BUCKETS {
            return Err(BudgetError::BucketsFull);
        }
        let reservation = keys
            .iter()
            .map(|key| {
                let bucket = self.buckets.entry(key.clone()).or_insert_with(|| Bucket {
                    generation: uuid::Uuid::new_v4().to_string(),
                    count: 0,
                    reserved_at: now_ms,
                    completed_at: None,
                });
                bucket.count += 1;
                bucket.reserved_at = now_ms;
                Reservation {
                    key: key.clone(),
                    generation: bucket.generation.clone(),
                }
            })
            .collect();
        Ok(ReserveOutcome {
            wait_ms: 0,
            reservation: Some(reservation),
        })
    }

    pub(crate) fn complete(
        &mut self,
        binding: &Binding,
        reservation: &[Reservation],
        now_ms: u64,
    ) -> Result<(), BudgetError> {
        self.validate(binding, now_ms)?;
        for entry in reservation {
            if self
                .buckets
                .get(&entry.key)
                .is_none_or(|bucket| bucket.generation != entry.generation)
            {
                return Err(BudgetError::Reservation);
            }
        }
        for entry in reservation {
            self.buckets
                .get_mut(&entry.key)
                .ok_or(BudgetError::Reservation)?
                .completed_at = Some(now_ms);
        }
        self.observed_at = now_ms;
        Ok(())
    }
}

pub(crate) fn login_keys(
    tenant_id: &str,
    username: &str,
    address: &str,
) -> Result<[String; 2], BudgetError> {
    let username = username.trim_matches(js_whitespace).to_lowercase();
    let parts = address.split('.').collect::<Vec<_>>();
    if tenant_id.is_empty()
        || username.is_empty()
        || parts.len() != 4
        || parts[0] != "198"
        || !matches!(parts[1], "18" | "19")
        || parts[2..].iter().any(|part| {
            !(1..=3).contains(&part.len()) || !part.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return Err(BudgetError::Identity);
    }
    Ok([
        format!("principal:{}", digest(&[tenant_id, &username])?),
        format!("ip:{}", digest(&[address])?),
    ])
}

fn digest(parts: &[&str]) -> Result<String, BudgetError> {
    let bytes = serde_json::to_vec(parts).map_err(|_| BudgetError::Identity)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

pub(crate) fn validate_time(now_ms: u64) -> Result<(), BudgetError> {
    if now_ms > MAX_SAFE_INTEGER {
        Err(BudgetError::Clock)
    } else {
        Ok(())
    }
}

fn integer(value: &Value) -> Option<u64> {
    let number = value.as_f64()?;
    (number.is_finite()
        && number.fract() == 0.0
        && (0.0..=MAX_SAFE_INTEGER as f64).contains(&number))
    .then_some(number as u64)
}

fn safe_integer<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    integer(&Value::deserialize(deserializer)?)
        .ok_or_else(|| de::Error::custom("登录预算必须使用非负安全整数"))
}

fn optional_integer<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u64>, D::Error> {
    let value = Value::deserialize(deserializer)?;
    if value.is_null() {
        Ok(None)
    } else {
        integer(&value)
            .map(Some)
            .ok_or_else(|| de::Error::custom("登录预算时间无效"))
    }
}

fn valid_key(key: &str) -> bool {
    key.strip_prefix("principal:")
        .or_else(|| key.strip_prefix("ip:"))
        .is_some_and(|digest| digest.len() == 64 && digest.bytes().all(hex_byte))
}

fn hex_byte(byte: u8) -> bool {
    byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')
}

fn expires_at(bucket: &Bucket, binding: &Binding) -> f64 {
    // 保留 JavaScript 的逐次浮点加法，包括安全整数上界附近的舍入。
    bucket.reserved_at.max(bucket.completed_at.unwrap_or(0)) as f64
        + binding.window_ms as f64
        + MARGIN_MS as f64
}

fn js_whitespace(character: char) -> bool {
    matches!(character, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}'
        | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}'
        | '\u{3000}' | '\u{feff}')
}

#[cfg(test)]
#[path = "../../tests/internal/login_budget/model.rs"]
mod tests;

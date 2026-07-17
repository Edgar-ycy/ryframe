use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use super::{Cache, CacheError};

#[derive(Deserialize, Serialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
enum ProtectedEntry {
    Value(Value),
    Null,
}

pub(super) enum CacheLookup<T> {
    Miss,
    Null,
    Value(T),
}

impl<T> CacheLookup<T> {
    pub(super) fn into_option(self) -> Option<T> {
        match self {
            Self::Value(value) => Some(value),
            Self::Miss | Self::Null => None,
        }
    }
}

pub(super) async fn read<C, T>(cache: &C, key: &str) -> Result<CacheLookup<T>, CacheError>
where
    C: Cache,
    T: DeserializeOwned + Send,
{
    match cache.get::<ProtectedEntry>(key).await? {
        Some(ProtectedEntry::Value(value)) => serde_json::from_value(value)
            .map(CacheLookup::Value)
            .map_err(|error| CacheError::Deserialize(error.to_string())),
        Some(ProtectedEntry::Null) => Ok(CacheLookup::Null),
        None => Ok(CacheLookup::Miss),
    }
}

pub(super) async fn write_value<C, T>(
    cache: &C,
    key: &str,
    value: &T,
    ttl_secs: u64,
) -> Result<(), CacheError>
where
    C: Cache,
    T: Serialize + Send + Sync,
{
    let value =
        serde_json::to_value(value).map_err(|error| CacheError::Serialize(error.to_string()))?;
    cache
        .set(key, &ProtectedEntry::Value(value), ttl_secs)
        .await
}

pub(super) async fn write_null<C: Cache>(
    cache: &C,
    key: &str,
    ttl_secs: u64,
) -> Result<(), CacheError> {
    cache.set(key, &ProtectedEntry::Null, ttl_secs).await
}

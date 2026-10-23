//! 恢复工具的原生 S3 端口；不发现环境凭据、不重定向、不自动重试。

use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use serde_json::Value;
use url::Url;

use crate::Result;

pub(crate) mod app_configuration;
mod client;
pub(crate) mod protocol;
mod response;
mod signing;
#[cfg(test)]
mod tests;
mod write;

pub(crate) struct Configuration {
    endpoint: Url,
    region: String,
    access_key: String,
    secret_key: String,
}

impl Configuration {
    pub(crate) fn read(value: &Value) -> Result<Self> {
        super::json::exact(
            value,
            &["endpoint", "region", "access_key", "secret_key"],
            "对象文件配置",
        )?;
        let field = |name| {
            value
                .get(name)
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty() && !text.chars().any(char::is_control))
                .map(ToOwned::to_owned)
                .ok_or_else(|| format!("对象文件配置字段 {name} 缺失或无效；请修正绑定配置文件"))
        };
        let endpoint = Url::parse(&field("endpoint")?)
            .map_err(|_| "对象 endpoint 无效；应为完整 HTTP(S) 地址")?;
        if !matches!(endpoint.scheme(), "http" | "https")
            || endpoint.host().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || endpoint.path() != "/"
        {
            return Err("对象 endpoint 必须为无凭据、查询和路径前缀的 HTTP(S) 服务地址".into());
        }
        let region = field("region")?;
        if !region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err("对象 region 只允许小写字母、数字和连字符".into());
        }
        Ok(Self {
            endpoint,
            region,
            access_key: field("access_key")?,
            secret_key: field("secret_key")?,
        })
    }
}

pub(crate) enum Operation {
    CreateBucket,
    HeadBucket,
    Versioning,
    List {
        prefix: String,
        continuation: Option<String>,
    },
    Head {
        key: String,
        if_match: Option<String>,
        version: Option<String>,
    },
    Get {
        key: String,
        if_match: Option<String>,
        version: Option<String>,
        destination: PathBuf,
        max_bytes: u64,
    },
}

pub(crate) struct Request {
    pub(crate) configuration: Configuration,
    pub(crate) bucket: String,
    pub(crate) operation: Operation,
}

impl Request {
    fn prepare(&self) -> Result<(reqwest::Method, Url, BTreeMap<String, String>)> {
        let (method, key) = match &self.operation {
            Operation::CreateBucket => (reqwest::Method::PUT, None),
            Operation::HeadBucket => (reqwest::Method::HEAD, None),
            Operation::Head { key, .. } => (reqwest::Method::HEAD, Some(key)),
            Operation::Get { key, .. } => (reqwest::Method::GET, Some(key)),
            _ => (reqwest::Method::GET, None),
        };
        let mut url = object_url(&self.configuration, &self.bucket, key.map(String::as_str))?;
        let mut headers = BTreeMap::new();
        let mut query = Vec::new();
        match &self.operation {
            Operation::Versioning => {
                query.push(("versioning", String::new()));
            }
            Operation::List {
                prefix,
                continuation,
            } => {
                query.extend([
                    ("list-type", "2".into()),
                    ("max-keys", "1000".into()),
                    ("encoding-type", "url".into()),
                    ("prefix", prefix.clone()),
                ]);
                if let Some(token) = continuation {
                    query.push(("continuation-token", token.clone()));
                }
            }
            Operation::Head {
                if_match, version, ..
            }
            | Operation::Get {
                if_match, version, ..
            } => {
                if let Some(value) = if_match {
                    headers.insert("if-match".into(), value.clone());
                }
                if let Some(value) = version {
                    query.push(("versionId", value.clone()));
                }
            }
            _ => {}
        }
        if !query.is_empty() {
            let query = query
                .into_iter()
                .map(|(key, value)| format!("{}={}", encode(key, false), encode(&value, false)))
                .collect::<Vec<_>>()
                .join("&");
            url.set_query(Some(&query));
        }
        Ok((method, url, headers))
    }
}

pub(crate) struct PutRequest {
    pub(crate) configuration: Configuration,
    pub(crate) bucket: String,
    pub(crate) key: String,
    pub(crate) source: PathBuf,
    pub(crate) expected_bytes: u64,
    pub(crate) expected_sha256: String,
    pub(crate) metadata: Value,
}

pub(crate) fn execute(request: Request) -> Result<Value> {
    crate::http_session::install_crypto_provider()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(client::execute(request, Duration::from_secs(1800)))
}

/// 创建登记桶只返回封闭结果；调用方必须随后独立 HEAD 核验，未知结果禁止重放。
pub(crate) fn create_bucket(configuration: Configuration, bucket: String) -> Result<Value> {
    crate::http_session::install_crypto_provider()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(client::create_bucket(
        Request {
            configuration,
            bucket,
            operation: Operation::CreateBucket,
        },
        Duration::from_secs(1800),
    ))
}

/// 条件创建只返回可持久化的明确状态；未知写入结果由上层账本核对，禁止自动重试。
pub(crate) fn put_if_absent(request: PutRequest) -> Result<Value> {
    crate::http_session::install_crypto_provider()?;
    let prepared = write::prepare(request)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(client::put_if_absent(prepared, Duration::from_secs(1800)))
}

fn object_url(configuration: &Configuration, bucket: &str, key: Option<&str>) -> Result<Url> {
    if bucket.is_empty()
        || !bucket.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.')
        })
    {
        return Err("对象桶名称无效".into());
    }
    let mut url = configuration.endpoint.clone();
    let mut path = format!("/{bucket}");
    if let Some(key) = key {
        // URL 库会折叠独立点段，拒绝这种不能原样传输的 key，不能写错对象。
        if key.is_empty() || key.split('/').any(|segment| matches!(segment, "." | "..")) {
            return Err("对象 key 为空或含独立点段，当前 HTTP 传输不能安全保持其身份".into());
        }
        path.push('/');
        path.push_str(&encode(key, true));
    }
    url.set_path(&path);
    Ok(url)
}

fn encode(value: &str, slash: bool) -> String {
    let mut result = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~')
            || (slash && byte == b'/')
        {
            result.push(char::from(byte));
        } else {
            use std::fmt::Write;
            let _ = write!(result, "%{byte:02X}");
        }
    }
    result
}

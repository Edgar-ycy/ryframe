use super::model;
use crate::{
    Result,
    http_session::{RequestHook, RequestMetadata, Session},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Duration};

#[path = "session/activation.rs"]
mod activation;

pub(super) struct Pacer {
    interval: Duration,
    previous: Option<tokio::time::Instant>,
}
impl Pacer {
    pub fn new(milliseconds: u64) -> Self {
        Self {
            interval: Duration::from_millis(milliseconds),
            previous: None,
        }
    }
    pub async fn wait(&mut self) {
        if let Some(previous) = self.previous {
            tokio::time::sleep_until(previous + self.interval).await;
        }
        self.previous = Some(tokio::time::Instant::now());
    }
}
#[async_trait::async_trait]
impl RequestHook for Pacer {
    async fn before(
        &mut self,
        _: &RequestMetadata,
    ) -> std::result::Result<(), crate::http_session::SessionError> {
        self.wait().await;
        Ok(())
    }
    async fn complete(&mut self) -> std::result::Result<(), crate::http_session::SessionError> {
        Ok(())
    }
}
pub(super) struct Client {
    session: Session,
    pub pacer: Pacer,
    http: reqwest::Client,
    api: url::Url,
    origin: String,
    identity: Value,
    address: String,
    catalog: BTreeMap<String, Value>,
}
impl Client {
    pub fn new(
        plan: &Value,
        side: &str,
        index: usize,
        catalog: &BTreeMap<String, Value>,
        password: &str,
    ) -> Result<Self> {
        Self::at(
            &plan[side],
            model::identity(plan, index),
            if side == "source" { 18 } else { 19 },
            index,
            catalog,
            password,
            plan["dataset"]["request_interval_ms"]
                .as_u64()
                .ok_or("请求节奏缺失")?,
        )
    }

    pub fn at(
        target: &Value,
        mut identity: Value,
        network: u8,
        index: usize,
        catalog: &BTreeMap<String, Value>,
        password: &str,
        request_interval_ms: u64,
    ) -> Result<Self> {
        if index > 10 || !matches!(network, 18 | 19) {
            return Err("参考身份序号或网络无效".into());
        }
        let api = model::endpoint(&target["api_url"])?;
        let frontend = model::endpoint(&target["frontend_url"])?;
        if identity["tenant_id"].as_str().is_none_or(str::is_empty)
            || identity["username"].as_str().is_none_or(str::is_empty)
        {
            return Err("参考身份缺少租户或用户名".into());
        }
        identity["password"] = json!(password);
        let address = format!("198.{network}.20.{}", index + 1);
        identity["client_address"] = json!(address);
        let config = json!({"bindings":{"api_url":api.as_str(),"frontend_url":frontend.as_str()},"contract":{"timeout_ms":120_000}});
        let session = Session::new(&config, catalog.clone(), &identity)?;
        let http = reqwest::Client::builder()
            .tls_backend_rustls()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|_| "无法创建数据集HTTP客户端")?;
        Ok(Self {
            session,
            pacer: Pacer::new(request_interval_ms),
            http,
            api,
            origin: frontend.origin().ascii_serialization(),
            identity,
            address,
            catalog: catalog.clone(),
        })
    }
    pub async fn login(&mut self) -> Result<()> {
        self.session.login(&mut self.pacer).await?;
        self.context(false).await?;
        Ok(())
    }
    pub async fn request(&mut self, step: &Value) -> Result<Value> {
        self.session
            .request(step, &mut self.pacer)
            .await
            .map_err(Into::into)
    }
    pub fn token(&self) -> Result<&str> {
        self.session
            .token()
            .ok_or_else(|| "数据集会话未认证".into())
    }
    pub fn clear_bearer(&mut self) {
        self.session.clear_access_token();
    }
    fn request_for(&self, operation: &str, method: &str) -> Result<reqwest::RequestBuilder> {
        let entry = self
            .catalog
            .get(operation)
            .ok_or("缺少数据集operation契约")?;
        let path = entry["path"].as_str().ok_or("operation路径缺失")?;
        if entry["method"] != method
            || !path.starts_with('/')
            || path.starts_with("//")
            || path.contains(['?', '#', '{', '}'])
        {
            return Err("数据集operation方法或路径不匹配".into());
        }
        let url = self.api.join(path)?;
        if url.origin() != self.api.origin() {
            return Err("数据集请求越过绑定源".into());
        }
        Ok(self
            .http
            .request(reqwest::Method::from_bytes(method.as_bytes())?, url)
            .bearer_auth(self.session.token().ok_or("数据集会话未认证")?)
            .header(
                "X-Tenant-Id",
                self.identity["tenant_id"].as_str().ok_or("租户身份缺失")?,
            )
            .header("X-Forwarded-For", &self.address)
            .header("Origin", &self.origin))
    }
    pub async fn upload(&mut self, filename: String, content: Vec<u8>) -> Result<Value> {
        let part = reqwest::multipart::Part::bytes(content)
            .file_name(filename)
            .mime_str("text/plain")?;
        let request = self
            .request_for("post_common_upload", "POST")?
            .header("Idempotency-Key", uuid::Uuid::new_v4().to_string())
            .multipart(reqwest::multipart::Form::new().part("file", part));
        self.pacer.wait().await;
        let mut response = request
            .send()
            .await
            .map_err(|_| "对象上传未确认；禁止自动重试")?;
        if !response.status().is_success() {
            return Err(
                format!("对象上传HTTP {}；禁止自动重试", response.status().as_u16()).into(),
            );
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "对象上传响应中断；禁止自动重试")?
        {
            if bytes.len() + chunk.len() > 1024 * 1024 {
                return Err("对象上传响应超过上限".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        let result: Value = serde_json::from_slice(&bytes).map_err(|_| "对象上传响应不是JSON")?;
        if result.get("code").is_some_and(|v| v != 200) {
            return Err("对象上传业务响应失败".into());
        }
        Ok(result)
    }
    pub async fn download(&mut self, file: &Value) -> Result<()> {
        use sha2::{Digest, Sha256};
        let expected = model::integer(&file["bytes"], 1, 4 * 1024 * 1024, "下载大小")?;
        let mut request = self
            .request_for("get_common_file_download", "GET")?
            .build()?;
        request
            .url_mut()
            .query_pairs_mut()
            .append_pair("bucket", "uploads")
            .append_pair("path", file["file_path"].as_str().ok_or("文件路径缺失")?);
        self.pacer.wait().await;
        let mut response = self
            .http
            .execute(request)
            .await
            .map_err(|_| "已有对象下载失败")?;
        if !response.status().is_success() {
            return Err("已有对象无法通过认证业务下载".into());
        }
        let mut received = 0u64;
        let mut digest = Sha256::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| "已有对象下载中断")? {
            received += chunk.len() as u64;
            if received > expected {
                return Err("已有对象响应超过声明大小".into());
            }
            digest.update(&chunk);
        }
        if received != expected || file["sha256"] != hex::encode(digest.finalize()) {
            return Err("已有对象下载内容不同".into());
        }
        Ok(())
    }
}
pub(super) fn catalog(root: &std::path::Path) -> Result<BTreeMap<String, Value>> {
    let document = super::documents::read(&root.join("openapi/openapi.json"))?;
    crate::devex::runtime_configuration::operation_catalog(&document).map_err(Into::into)
}

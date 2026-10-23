//! 恢复运行的只读在线探针；不重试、不跟随重定向、不改变运行代次。
use std::time::Duration;

use reqwest::{Client, Url};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{Result, recovery::clone::native::json as strict};

#[cfg(test)]
#[path = "../../../tests/unit/recovery/runtime/probes/tests.rs"]
mod tests;

pub(crate) fn request(args: &Value) -> Result<Value> {
    // 先校验整个请求，再进行任何网络观察。
    validate(args)?;
    crate::http_session::install_crypto_provider()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(controlled(args, async {
        let _ = tokio::signal::ctrl_c().await;
    }))
}

async fn controlled(args: &Value, cancel: impl std::future::Future<Output = ()>) -> Result<Value> {
    tokio::select! {
        result = observe(args) => result,
        () = cancel => Err("恢复运行探针已取消；未执行远端写入".into()),
    }
}

fn validate(args: &Value) -> Result<()> {
    let role = args["role"].as_str().ok_or("恢复运行探针角色无效")?;
    strict::exact(
        args,
        if role == "frontend" {
            &["role", "url", "files"]
        } else {
            &["role", "url"]
        },
        "恢复运行探针",
    )?;
    if !matches!(role, "api" | "worker" | "frontend") {
        return Err("恢复运行探针角色无效".into());
    }
    let url = endpoint(args)?;
    if role == "frontend" {
        if url.path() != "/" {
            return Err("恢复前端端点必须为无路径的HTTP来源".into());
        }
        let files = args["files"].as_array().ok_or("恢复前端文件清单无效")?;
        let mut paths = std::collections::BTreeSet::new();
        for file in files {
            strict::exact(file, &["path", "bytes", "sha256"], "恢复前端文件")?;
            let path = file["path"].as_str().ok_or("恢复前端文件路径无效")?;
            if path.is_empty()
                || path.contains(['\\', '\0', '\r', '\n'])
                || path.split('/').any(|part| matches!(part, "" | "." | ".."))
                || !paths.insert(path)
                || file["bytes"].as_u64().is_none()
                || !valid_digest(file["sha256"].as_str().unwrap_or_default())
            {
                return Err("恢复前端文件路径、长度或摘要无效".into());
            }
        }
        if !paths.contains("index.html") || !paths.contains(".vite/manifest.json") {
            return Err("恢复前端缺少生产首页或 Vite manifest".into());
        }
    }
    Ok(())
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn endpoint(args: &Value) -> Result<Url> {
    let raw = args["url"].as_str().ok_or("恢复运行探针端点无效")?;
    let url = Url::parse(raw).map_err(|_| "恢复运行探针端点不是有效URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || raw.contains(['\0', '\r', '\n'])
    {
        return Err("恢复运行探针端点包含不支持的协议、凭据、查询或片段".into());
    }
    Ok(url)
}

async fn observe(args: &Value) -> Result<Value> {
    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(Duration::from_secs(10))
        .build()?;
    let url = endpoint(args)?;
    match args["role"].as_str() {
        Some("api") => {
            let (mime, body) = response(&client, url, 64 * 1024, true).await?;
            if !mime
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .eq_ignore_ascii_case("application/json")
            {
                return Err("API readyz 必须返回 application/json".into());
            }
            readiness(&body)
        }
        Some("worker") => {
            response(&client, url, 0, false).await?;
            Ok(json!({"status":"verified"}))
        }
        Some("frontend") => frontend(&client, url, &args["files"]).await,
        _ => Err("恢复运行探针角色无效".into()),
    }
}

async fn response(client: &Client, url: Url, limit: u64, api: bool) -> Result<(String, Vec<u8>)> {
    let mut response = client
        .get(url)
        .header("Accept-Encoding", "identity")
        .header("Accept", if api { "application/json" } else { "*/*" })
        .send()
        .await
        .map_err(|_| "恢复探针连接失败或读取超时")?;
    if response.status().as_u16() != 200 {
        return Err("恢复探针拒绝非200响应或重定向".into());
    }
    let mime = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "恢复探针响应读取失败或超时")?
    {
        if chunk.len() as u64 > limit.saturating_sub(body.len() as u64) {
            return Err("恢复探针响应体超过登记上限".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok((mime, body))
}

fn readiness(bytes: &[u8]) -> Result<Value> {
    let text = std::str::from_utf8(bytes).map_err(|_| "API readyz 必须返回UTF-8严格JSON")?;
    let value = strict::parse_fragment(text, "API readyz")
        .map_err(|_| "API readyz 必须返回唯一字段的严格JSON")?;
    strict::exact(
        &value,
        &["status", "mysql", "redis", "object_storage"],
        "API readyz",
    )?;
    if value["status"] != "ready"
        || value["mysql"] != "up"
        || !matches!(value["redis"].as_str(), Some("up" | "optional_degraded"))
        || !matches!(
            value["object_storage"].as_str(),
            Some("up" | "not_required")
        )
    {
        return Err("API readyz 没有证明必要依赖已就绪".into());
    }
    Ok(value)
}

async fn frontend(client: &Client, base: Url, files: &Value) -> Result<Value> {
    for file in files.as_array().ok_or("恢复前端文件清单无效")? {
        let path = file["path"].as_str().ok_or("恢复前端文件路径无效")?;
        if path.starts_with(".vite/") {
            continue;
        }
        let mut url = base.clone();
        url.path_segments_mut()
            .map_err(|_| "恢复前端端点不能包含相对来源")?
            .clear()
            .extend(path.split('/'));
        let expected = file["bytes"].as_u64().ok_or("恢复前端文件长度无效")?;
        let (_, body) = response(client, url, expected, false).await?;
        if body.len() as u64 != expected || hex::encode(Sha256::digest(&body)) != file["sha256"] {
            return Err("恢复站点实际返回的资源与生产构建不一致".into());
        }
    }
    Ok(json!({"status":"verified"}))
}

use std::{fs::OpenOptions, io::Write, time::Duration};

use chrono::Utc;
use futures_util::stream;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use super::{Operation, Request, response, signing, write};
use crate::Result;

pub(super) async fn execute(request: Request, timeout: Duration) -> Result<Value> {
    let (method, url, mut headers) = request.prepare()?;
    let configuration = &request.configuration;
    let payload_hash = hex::encode(Sha256::digest([]));
    let date = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let host = match url.port() {
        Some(port) => format!("{}:{port}", url.host().ok_or("对象端点缺少 host")?),
        None => url.host().ok_or("对象端点缺少 host")?.to_string(),
    };
    headers.insert("host".into(), host);
    headers.insert("x-amz-date".into(), date.clone());
    headers.insert("x-amz-content-sha256".into(), payload_hash.clone());
    let authorization = signing::authorization(signing::SigningInput {
        method: method.as_str(),
        url: &url,
        headers: &headers,
        payload_hash: &payload_hash,
        access_key: &configuration.access_key,
        secret_key: &configuration.secret_key,
        region: &configuration.region,
        amz_date: &date,
    })?;
    let client = http_client(timeout)?;
    let mut builder = client.request(method, url);
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    let output = builder
        .header("authorization", authorization)
        .send()
        .await
        .map_err(|error| {
            format!(
                "原生 S3 请求失败（{}）；未执行写请求，请检查端点、TLS 与网络",
                if error.is_timeout() {
                    "timeout"
                } else if error.is_connect() {
                    "connect"
                } else {
                    "transport"
                }
            )
        })?;
    if output.status() == reqwest::StatusCode::NOT_FOUND
        && matches!(&request.operation, Operation::Head { .. })
    {
        return Ok(serde_json::json!({
            "status": "not_found",
            "http_status": 404,
            "remote_write_may_have_occurred": false,
        }));
    }
    if output.status() != reqwest::StatusCode::OK {
        // 服务端正文可能回显认证信息；诊断只记录状态和有限的请求标识。
        return Err(format!(
            "原生 S3 请求被拒绝，HTTP {}；请检查文件中的访问权限、区域及条件版本；未执行写请求",
            output.status().as_u16()
        )
        .into());
    }
    read_output(output, &request.operation).await
}

pub(super) async fn put_if_absent(request: write::Prepared, timeout: Duration) -> Result<Value> {
    let mut headers = request.headers.clone();
    let date = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let host = match request.url.port() {
        Some(port) => format!("{}:{port}", request.url.host().ok_or("对象端点缺少 host")?),
        None => request.url.host().ok_or("对象端点缺少 host")?.to_string(),
    };
    headers.insert("host".into(), host);
    headers.insert("x-amz-date".into(), date.clone());
    headers.insert(
        "x-amz-content-sha256".into(),
        request.expected_sha256.clone(),
    );
    let authorization = signing::authorization(signing::SigningInput {
        method: "PUT",
        url: &request.url,
        headers: &headers,
        payload_hash: &request.expected_sha256,
        access_key: &request.configuration.access_key,
        secret_key: &request.configuration.secret_key,
        region: &request.configuration.region,
        amz_date: &date,
    })?;
    let file = tokio::fs::File::open(&request.source)
        .await
        .map_err(|_| "无法打开条件创建来源；未执行写请求")?;
    let stream = stream::try_unfold(
        (file, request.expected_bytes),
        |(mut file, remaining)| async move {
            if remaining == 0 {
                return Ok::<_, std::io::Error>(None);
            }
            let capacity = remaining.min(64 * 1024) as usize;
            let mut buffer = vec![0_u8; capacity];
            let count = file.read(&mut buffer).await?;
            if count == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "条件创建来源在发送期间缩短",
                ));
            }
            buffer.truncate(count);
            Ok(Some((buffer, (file, remaining - count as u64))))
        },
    );
    let client = http_client(timeout)?;
    let mut builder = client
        .put(request.url.clone())
        .body(reqwest::Body::wrap_stream(stream));
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    let output = match builder.header("authorization", authorization).send().await {
        Ok(value) => value,
        Err(error) => {
            return Ok(serde_json::json!({
                "status": "needs_reconciliation",
                "reason": if error.is_timeout() { "timeout" } else if error.is_connect() { "connect" } else { "transport" },
                "remote_write_may_have_occurred": true,
                "automatic_retry_permitted": false,
            }));
        }
    };
    let status = output.status();
    if write::verify_prepared_source(&request).is_err() {
        return Ok(serde_json::json!({
            "status": "needs_reconciliation",
            "reason": "source_changed",
            "http_status": status.as_u16(),
            "remote_write_may_have_occurred": true,
            "automatic_retry_permitted": false,
        }));
    }
    write_outcome(output)
}

pub(super) async fn create_bucket(request: Request, timeout: Duration) -> Result<Value> {
    let (method, url, mut headers) = request.prepare()?;
    let configuration = &request.configuration;
    let payload_hash = hex::encode(Sha256::digest([]));
    let date = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let host = match url.port() {
        Some(port) => format!("{}:{port}", url.host().ok_or("对象端点缺少 host")?),
        None => url.host().ok_or("对象端点缺少 host")?.to_string(),
    };
    headers.insert("host".into(), host);
    headers.insert("x-amz-date".into(), date.clone());
    headers.insert("x-amz-content-sha256".into(), payload_hash.clone());
    let authorization = signing::authorization(signing::SigningInput {
        method: method.as_str(),
        url: &url,
        headers: &headers,
        payload_hash: &payload_hash,
        access_key: &configuration.access_key,
        secret_key: &configuration.secret_key,
        region: &configuration.region,
        amz_date: &date,
    })?;
    let client = http_client(timeout)?;
    let mut builder = client.request(method, url);
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    let output = match builder.header("authorization", authorization).send().await {
        Ok(value) => value,
        Err(error) => {
            return Ok(serde_json::json!({
                "status": "needs_reconciliation",
                "reason": if error.is_timeout() { "timeout" } else if error.is_connect() { "connect" } else { "transport" },
                "remote_write_may_have_occurred": true,
                "automatic_retry_permitted": false,
            }));
        }
    };
    let status = output.status();
    let base = serde_json::json!({
        "http_status": status.as_u16(),
        "automatic_retry_permitted": false,
    });
    let state = match status.as_u16() {
        200 => serde_json::json!({
            "status": "created_unverified",
            "remote_write_may_have_occurred": true,
        }),
        409 => serde_json::json!({
            "status": "already_exists",
            "remote_write_may_have_occurred": false,
        }),
        401 | 403 => serde_json::json!({
            "status": "denied",
            "remote_write_may_have_occurred": false,
        }),
        _ => serde_json::json!({
            "status": "needs_reconciliation",
            "remote_write_may_have_occurred": true,
        }),
    };
    Ok(merge(base, state))
}

fn http_client(timeout: Duration) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .tls_backend_rustls()
        .no_proxy()
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(15))
        .timeout(timeout)
        .build()
        .map_err(|_| "原生 S3 HTTP 客户端创建失败；请检查 TLS 配置".into())
}

fn write_outcome(output: reqwest::Response) -> Result<Value> {
    let status = output.status();
    let base = serde_json::json!({
        "http_status": status.as_u16(),
        "automatic_retry_permitted": false,
    });
    if status == reqwest::StatusCode::PRECONDITION_FAILED {
        return Ok(merge(
            base,
            serde_json::json!({
                "status": "already_exists",
                "remote_write_may_have_occurred": false,
            }),
        ));
    }
    if matches!(status.as_u16(), 401 | 403) {
        return Ok(merge(
            base,
            serde_json::json!({
                "status": "denied",
                "remote_write_may_have_occurred": false,
            }),
        ));
    }
    if status != reqwest::StatusCode::OK {
        return Ok(merge(
            base,
            serde_json::json!({
                "status": "needs_reconciliation",
                "remote_write_may_have_occurred": true,
            }),
        ));
    }
    let etags = output.headers().get_all("etag").iter().collect::<Vec<_>>();
    let etag = etags
        .first()
        .filter(|_| etags.len() == 1)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty());
    let Some(etag) = etag else {
        return Ok(merge(
            base,
            serde_json::json!({
                "status": "needs_reconciliation",
                "reason": "success_response_incomplete",
                "remote_write_may_have_occurred": true,
            }),
        ));
    };
    let mut response = serde_json::json!({"ETag": etag});
    if let Some(version) = output
        .headers()
        .get("x-amz-version-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
    {
        response["VersionId"] = serde_json::json!(version);
    }
    Ok(merge(
        base,
        serde_json::json!({
            "status": "created_unverified",
            "response": response,
            "remote_write_may_have_occurred": true,
        }),
    ))
}

fn merge(mut left: Value, right: Value) -> Value {
    if let (Some(left), Some(right)) = (left.as_object_mut(), right.as_object()) {
        left.extend(right.clone());
    }
    left
}

async fn read_output(mut output: reqwest::Response, operation: &Operation) -> Result<Value> {
    match operation {
        Operation::CreateBucket => Err("创建桶响应必须由写入状态处理器消费".into()),
        Operation::HeadBucket => Ok(serde_json::json!({})),
        Operation::Head { .. } => response::metadata(output.headers()),
        Operation::Get {
            destination,
            max_bytes,
            ..
        } => {
            let metadata = response::metadata(output.headers())?;
            let expected = metadata["ContentLength"]
                .as_u64()
                .ok_or("S3 GET 缺少完整长度")?;
            if expected > *max_bytes {
                return Err("S3 GET 超出本次已声明的字节上限，未创建对象文件".into());
            }
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(destination)
                .map_err(|_| "S3 GET 输出无法独占创建；请检查隔离目录和已有证据，不能覆盖后重试")?;
            let mut bytes = 0_u64;
            while let Some(chunk) = output
                .chunk()
                .await
                .map_err(|_| "S3 GET 流中断；已保留部分文件，不得作为完整对象使用")?
            {
                bytes = bytes
                    .checked_add(chunk.len() as u64)
                    .ok_or("S3 GET 长度溢出")?;
                if bytes > expected {
                    return Err("S3 GET 实际长度超过声明值；保留部分文件以便核查".into());
                }
                file.write_all(&chunk)?;
            }
            file.sync_all()?;
            if bytes != expected {
                return Err("S3 GET 长度不足；保留部分文件以便核查".into());
            }
            Ok(metadata)
        }
        Operation::Versioning | Operation::List { .. } => {
            let mut bytes = Vec::new();
            while let Some(chunk) = output.chunk().await.map_err(|_| "S3 XML 响应读取中断")? {
                if bytes.len().saturating_add(chunk.len()) > 16 * 1024 * 1024 {
                    return Err("S3 XML 响应超过 16 MiB 上限".into());
                }
                bytes.extend_from_slice(&chunk);
            }
            response::xml(&bytes, operation)
        }
    }
}

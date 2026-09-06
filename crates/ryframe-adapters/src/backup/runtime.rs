use ryframe_application::ports::backup::{
    BackupRuntimeVerifier, RestoreBusinessProof, RestoreRecord,
};
use ryframe_kernel::{AppError, AppResult};
use std::{sync::Arc, time::Duration};

pub fn runtime_verifier(
    api_ready_url: String,
    worker_ready_url: String,
) -> AppResult<Arc<dyn BackupRuntimeVerifier>> {
    validate_endpoint(&api_ready_url)?;
    validate_endpoint(&worker_ready_url)?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| AppError::Config("无法建立恢复就绪校验客户端".into()))?;
    Ok(Arc::new(RuntimeVerification {
        client,
        api_ready_url,
        worker_ready_url,
    }))
}

fn validate_endpoint(endpoint: &str) -> AppResult<()> {
    let parsed = reqwest::Url::parse(endpoint)
        .map_err(|_| AppError::Config("恢复就绪端点不是有效 URL".into()))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(AppError::Config("恢复就绪端点只允许 HTTP 或 HTTPS".into()));
    }
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(AppError::Config(
            "恢复就绪端点不得包含凭据、查询或片段".into(),
        ));
    }
    Ok(())
}

struct RuntimeVerification {
    client: reqwest::Client,
    api_ready_url: String,
    worker_ready_url: String,
}

#[async_trait::async_trait]
impl BackupRuntimeVerifier for RuntimeVerification {
    async fn restored_runtime(
        &self,
        record: &RestoreRecord,
        _proof: &RestoreBusinessProof,
    ) -> AppResult<()> {
        if record.plan.api_ready_url != self.api_ready_url
            || record.plan.worker_ready_url != self.worker_ready_url
        {
            return Err(AppError::Validation(
                "API 或 Worker 端点与恢复配置不一致".into(),
            ));
        }
        for url in [&self.api_ready_url, &self.worker_ready_url] {
            let response = self
                .client
                .get(url)
                .send()
                .await
                .map_err(|_| AppError::Validation("恢复后的 API 或 Worker 无法连接".into()))?;
            if response.status() != reqwest::StatusCode::OK {
                return Err(AppError::Validation("恢复后的 API 或 Worker 未就绪".into()));
            }
        }
        Ok(())
    }
}

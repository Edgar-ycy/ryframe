use super::*;

pub(super) fn encode_success(
    request: &AgentRequest,
    data: &serde_json::Value,
    max_bytes: usize,
) -> AppResult<Vec<u8>> {
    let value = serde_json::json!({
        "code": 200,
        "message": request.success_message,
        "data": data,
        "request_id": request.request_id,
        "error_key": null,
        "details": null,
    });
    let encoded = serde_json::to_vec(&value)
        .map_err(|_| AppError::Internal("序列化 Agent 响应失败".into()))?;
    if encoded.len() > max_bytes {
        return Err(AppError::PayloadTooLarge(
            "Agent 响应超过大小上限，请缩小分页".into(),
        ));
    }
    Ok(encoded)
}

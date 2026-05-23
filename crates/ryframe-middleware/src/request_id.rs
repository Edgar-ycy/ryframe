use axum::{extract::Request, middleware::Next, response::Response};
use uuid::Uuid;

pub async fn request_id_middleware(mut request: Request, next: Next) -> Response {
    let request_id = Uuid::now_v7().to_string();
    request
        .extensions_mut()
        .insert(RequestId(request_id.clone()));

    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert("x-request-id", request_id.parse().unwrap());
    response
}

#[derive(Debug, Clone)]
pub struct RequestId(pub String);

use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    time::Duration,
};

use axum::{Router, middleware::from_fn_with_state};
use ryframe_api::{
    AppState,
    auth_middleware::auth_middleware,
    handlers::{
        profile_handler::profile_router, user_handler::user_router,
        user_import_handler::user_import_router,
    },
};

use super::message;

pub(super) struct Response {
    pub(super) status: u16,
    pub(super) body: serde_json::Value,
}
pub(super) struct Server {
    address: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Server {
    pub(super) async fn start(state: AppState) -> Result<Self, String> {
        let app = Router::new()
            .nest(
                "/api/v1/system/user-imports",
                user_import_router(state.clone()),
            )
            .nest("/api/v1/system/users", user_router(state.clone()))
            .nest(
                "/api/v1/auth/profile",
                profile_router().with_state(state.clone()),
            )
            .layer(from_fn_with_state(state.auth, auth_middleware));
        Self::start_router(app).await
    }

    pub(super) async fn start_router(app: Router) -> Result<Self, String> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(message)?;
        let address = listener.local_addr().map_err(message)?;
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Ok(Self { address, task })
    }

    pub(super) async fn get(&self, path: &str, token: &str) -> Result<Response, String> {
        self.request("GET", path, token, "", Vec::new()).await
    }

    pub(super) async fn change_password(
        &self,
        token: &str,
        old: &str,
        new: &str,
    ) -> Result<(), String> {
        let body = serde_json::to_vec(&serde_json::json!({"old_password":old,"new_password":new}))
            .map_err(message)?;
        let response = self
            .request(
                "PUT",
                "/api/v1/auth/profile/password",
                token,
                "Content-Type: application/json\r\n",
                body,
            )
            .await?;
        assert_eq!(response.status, 200);
        Ok(())
    }

    pub(super) async fn upload(&self, token: &str, workbook: Vec<u8>) -> Result<Response, String> {
        let mut body = b"--account-import-boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"accounts.xlsx\"\r\nContent-Type: application/vnd.openxmlformats-officedocument.spreadsheetml.sheet\r\n\r\n".to_vec();
        body.extend(workbook);
        body.extend_from_slice(b"\r\n--account-import-boundary--\r\n");
        self.request("POST", "/api/v1/system/user-imports", token,
            "Content-Type: multipart/form-data; boundary=account-import-boundary\r\nIdempotency-Key: imported-accounts-password-cycle\r\n", body).await
    }

    pub(super) async fn request(
        &self,
        method: &str,
        path: &str,
        token: &str,
        headers: &str,
        body: Vec<u8>,
    ) -> Result<Response, String> {
        let address = self.address;
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        tokio::task::spawn_blocking(move || {
            let mut stream =
                TcpStream::connect_timeout(&address, Duration::from_secs(10)).map_err(message)?;
            stream
                .set_read_timeout(Some(Duration::from_secs(30)))
                .map_err(message)?;
            stream
                .set_write_timeout(Some(Duration::from_secs(30)))
                .map_err(message)?;
            stream
                .write_all(head.as_bytes())
                .and_then(|_| stream.write_all(&body))
                .map_err(|_| "写入账号导入 HTTP 请求失败".to_owned())?;
            let mut bytes = Vec::new();
            stream
                .take(1024 * 1024)
                .read_to_end(&mut bytes)
                .map_err(message)?;
            let separator = bytes
                .windows(4)
                .position(|part| part == b"\r\n\r\n")
                .ok_or("账号导入 HTTP 响应缺少头边界")?;
            let head =
                std::str::from_utf8(&bytes[..separator]).map_err(|_| "账号导入 HTTP 响应头无效")?;
            let status = head
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .ok_or("账号导入 HTTP 响应缺少状态码")?
                .parse()
                .map_err(message)?;
            let body = serde_json::from_slice(&bytes[separator + 4..]).map_err(|_| {
                format!(
                    "账号导入 HTTP 响应不是完整 JSON：status={status}，body_bytes={}",
                    bytes.len() - separator - 4
                )
            })?;
            Ok(Response { status, body })
        })
        .await
        .map_err(message)?
    }
}

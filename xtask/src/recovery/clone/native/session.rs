//! 已绑定工具通过本地标准输入接收授权；授权到达前不读取密码或发送网络请求。

use std::io::{BufRead, Write};

use serde_json::{Value, json};

use crate::{
    http_session::{Session, SessionError},
    session_pacing::PreparationHook,
};

#[path = "session/model.rs"]
pub(crate) mod model;

use model::Registration;

pub(crate) fn run_hidden_if_requested() -> Option<i32> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments.first().map(String::as_str) != Some("__copy-session") {
        return None;
    }
    let result = (|| {
        let root = std::env::current_dir().map_err(|_| "InvalidSessionRoot")?;
        let registration = Registration::parse(&root, &arguments[1..])?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "SessionRuntimeUnavailable")?;
        runtime.block_on(run(
            &registration,
            &mut std::io::stdin().lock(),
            &mut std::io::stdout().lock(),
        ))
    })();
    Some(match result {
        Ok(()) => 0,
        Err(category) => {
            eprintln!(
                "原生复制会话失败：{category}；保留当前意图和收据，核对结果后显式恢复，禁止自动重放。"
            );
            1
        }
    })
}

pub(crate) async fn run(
    registration: &Registration,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> Result<(), &'static str> {
    let mut state = State::default();
    let mut sequence = 0_u64;
    let result = loop {
        let request = match read_request(input) {
            Ok(Some(value)) => value,
            Ok(None) => break Ok(()),
            Err(category) => break Err(category),
        };
        sequence += 1;
        if request["id"].as_u64() != Some(sequence) {
            let _ = send(
                output,
                &json!({"id":null,"ok":false,"error_type":"InvalidSessionSequence","status":null}),
            );
            break Err("InvalidSessionSequence");
        }
        let operation = request["operation"].as_str().unwrap_or("");
        let response = state.execute(registration, &request).await;
        match response {
            Ok(data) => {
                if let Err(category) = send(output, &json!({"id":sequence,"ok":true,"data":data})) {
                    break Err(category);
                }
                if operation == "close" {
                    break Ok(());
                }
            }
            Err(error) => {
                let _ = send(
                    output,
                    &json!({"id":sequence,"ok":false,"error_type":error.category,"status":error.status}),
                );
                break Err(error.category);
            }
        }
    };
    let cleanup = state.close().await.map_err(|error| error.category);
    result.and(cleanup)
}

fn read_request(input: &mut impl BufRead) -> Result<Option<Value>, &'static str> {
    let mut bytes = Vec::new();
    loop {
        let available = input.fill_buf().map_err(|_| "SessionInputUnavailable")?;
        if available.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err("IncompleteSessionRequest")
            };
        }
        let length = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|index| index + 1)
            .unwrap_or(available.len());
        if bytes.len() + length > 1024 * 1024 {
            return Err("SessionRequestTooLarge");
        }
        bytes.extend_from_slice(&available[..length]);
        input.consume(length);
        if bytes.last() == Some(&b'\n') {
            let text = std::str::from_utf8(&bytes).map_err(|_| "InvalidSessionEncoding")?;
            return super::json::parse_fragment(text, "原生会话请求")
                .map(Some)
                .map_err(|_| "InvalidSessionJson");
        }
    }
}

fn send(output: &mut impl Write, value: &Value) -> Result<(), &'static str> {
    serde_json::to_writer(&mut *output, value).map_err(|_| "SessionOutputUnavailable")?;
    output
        .write_all(b"\n")
        .and_then(|()| output.flush())
        .map_err(|_| "SessionOutputUnavailable")
}

fn error(category: &'static str) -> SessionError {
    SessionError {
        category,
        operation: None,
        status: None,
    }
}

#[derive(Default)]
struct State {
    session: Option<Session>,
    hook: Option<PreparationHook>,
    ready: bool,
    login_attempted: bool,
    logout_attempted: bool,
    closed: bool,
}

impl State {
    async fn execute(
        &mut self,
        registration: &Registration,
        request: &Value,
    ) -> Result<Value, SessionError> {
        match request["operation"].as_str() {
            Some("initialize") if self.session.is_none() && !self.closed => {
                self.initialize(registration, request).await
            }
            Some("get" | "disable") if self.ready && !self.closed => {
                let step = model::schedule(request).map_err(error)?;
                let response = self
                    .session
                    .as_mut()
                    .ok_or_else(|| error("MissingSession"))?
                    .request(
                        &step,
                        self.hook.as_mut().ok_or_else(|| error("MissingPacing"))?,
                    )
                    .await?;
                Ok(response["data"].clone())
            }
            Some("close") if self.ready && !self.closed => {
                model::exact(request, &["id", "operation"]).map_err(error)?;
                self.close().await?;
                Ok(json!({"closed":true}))
            }
            _ => Err(error("InvalidSessionOperation")),
        }
    }

    async fn initialize(
        &mut self,
        registration: &Registration,
        request: &Value,
    ) -> Result<Value, SessionError> {
        let artifacts = registration.initialize(request).map_err(error)?;
        let app_env = std::env::var("APP_ENV").map_err(|_| error("MissingSessionEnvironment"))?;
        if !matches!(app_env.as_str(), "dev" | "test") {
            return Err(error("InvalidSessionEnvironment"));
        }
        let document = super::json::read(&registration.root.join("openapi/openapi.json"))
            .map_err(|_| error("InvalidSessionCatalog"))?;
        let catalog = crate::devex::runtime_configuration::operation_catalog(&document)
            .map_err(|_| error("InvalidSessionCatalog"))?;
        let session = Session::new(&request["config"], catalog, &request["identity"])?;
        self.hook = Some(
            PreparationHook::new(
                &registration.root,
                &request["config"],
                &request["identity"],
                &artifacts,
                &app_env,
            )
            .await?,
        );
        self.session = Some(session);
        let session = self
            .session
            .as_mut()
            .ok_or_else(|| error("MissingSession"))?;
        let hook = self.hook.as_mut().ok_or_else(|| error("MissingPacing"))?;
        self.login_attempted = true;
        session.login(hook).await?;
        let response = session
            .request(&json!({"operation":"get_auth_context"}), hook)
            .await?;
        let auth = model::authorization(&response, &request["identity"]).map_err(error)?;
        self.ready = true;
        Ok(auth)
    }

    async fn close(&mut self) -> Result<(), SessionError> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        let mut result = Ok(());
        if let (Some(session), Some(hook)) = (&mut self.session, &mut self.hook)
            && (self.login_attempted || session.token().is_some())
            && !self.logout_attempted
        {
            self.logout_attempted = true;
            result = match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                session.request(&json!({"operation":"post_auth_logout"}), hook),
            )
            .await
            {
                Ok(value) => value.map(|_| ()),
                Err(_) => Err(error("SessionLogoutUnknown")),
            };
        }
        if let Some(hook) = &mut self.hook {
            let cleanup = hook.close().await;
            result = result.and(cleanup);
        }
        result
    }
}

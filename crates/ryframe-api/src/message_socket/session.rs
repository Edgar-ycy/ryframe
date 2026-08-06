use super::frame::handle_client_frame;
use super::*;

/// 申请一次性 WebSocket 票据后的 HTTP 响应。
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct WebSocketTicketResponse {
    pub ticket: String,
    pub expires_in: u64,
}

/// WebSocket 升级查询参数；原始票据只会在此处使用，日志不会记录查询字符串。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebSocketQuery {
    pub ticket: String,
}

/// 执行 Origin 校验、消费一次性票据并升级为消息连接。
pub async fn upgrade(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
    Query(query): Query<WebSocketQuery>,
    headers: HeaderMap,
) -> HttpResult<impl IntoResponse> {
    validate_websocket_origin(&state, &headers)?;
    let ticket = match state.services.websocket_ticket.consume(&query.ticket).await {
        Ok(ticket) => {
            ryframe_middleware::metrics::record_ws_ticket("consumed");
            ticket
        }
        Err(error) => {
            let result = if matches!(error, AppError::ServiceUnavailable(_)) {
                "backend_error"
            } else {
                "rejected"
            };
            ryframe_middleware::metrics::record_ws_ticket(result);
            return Err(error.into());
        }
    };
    state
        .services
        .auth
        .validate_websocket_session(
            &ticket.tenant_id,
            ticket.user_id,
            &ticket.session_id,
            ticket.user_authorization_version,
            ticket.tenant_session_version,
        )
        .await?;
    let hub = state.message_hub.clone();
    let service = state.services.message.clone();
    Ok(ws.on_upgrade(move |socket| handle_socket(socket, hub, service, ticket)))
}

async fn handle_socket(
    socket: WebSocket,
    hub: Arc<MessageHub>,
    service: Arc<MessageService>,
    ticket: WebSocketTicket,
) {
    let (outbound, mut outbound_receiver) = mpsc::channel(hub.config.outbound_buffer);
    let (shutdown_sender, mut shutdown_receiver) = watch::channel(false);
    let (mut socket_sender, mut socket_receiver) = socket.split();
    let Some(connection_id) = hub.try_register(&ticket, outbound.clone(), shutdown_sender.clone())
    else {
        ryframe_middleware::metrics::record_message_delivery("connection_limit");
        let _ = socket_sender
            .send(Message::Close(Some(CloseFrame {
                code: 1008,
                reason: "当前用户的 WebSocket 连接数已达上限".into(),
            })))
            .await;
        return;
    };

    if !hub.initialize_connection(&connection_id, &ticket) {
        hub.unregister(&connection_id);
        return;
    }

    let mut send_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                changed = shutdown_receiver.changed() => {
                    let should_close = changed.is_ok() && *shutdown_receiver.borrow();
                    if should_close {
                        let _ = socket_sender.send(Message::Close(Some(CloseFrame {
                            code: 1013,
                            reason: "客户端消费速度过慢".into(),
                        }))).await;
                        break;
                    }
                }
                message = outbound_receiver.recv() => match message {
                    Some(message) => {
                        let should_close = matches!(&message, Message::Close(_));
                        if socket_sender.send(message).await.is_err() || should_close {
                            break;
                        }
                    }
                    None => break,
                }
            }
        }
    });

    let outbound_for_receive = outbound.clone();
    let localizer_for_receive = hub.localizer.clone();
    let locale_for_receive = Locale::parse(&ticket.locale).unwrap_or(Locale::DEFAULT);
    let mut receive_task = tokio::spawn(async move {
        loop {
            match socket_receiver.next().await {
                Some(Ok(Message::Text(text))) => {
                    handle_client_frame(
                        &service,
                        &ticket,
                        &localizer_for_receive,
                        locale_for_receive,
                        &outbound_for_receive,
                        text.as_str(),
                    )
                    .await;
                }
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(Message::Ping(_)))
                | Some(Ok(Message::Pong(_)))
                | Some(Ok(Message::Binary(_))) => {}
                Some(Err(error)) => {
                    tracing::debug!(%error, "消息 WebSocket 接收失败");
                    break;
                }
            }
        }
    });

    let send_abort = send_task.abort_handle();
    let receive_abort = receive_task.abort_handle();
    tokio::select! {
        _ = &mut send_task => receive_abort.abort(),
        _ = &mut receive_task => send_abort.abort(),
    }
    hub.unregister(&connection_id);
    tracing::debug!(connection_id, "消息 WebSocket 连接已关闭");
}

fn validate_websocket_origin(state: &AppState, headers: &HeaderMap) -> HttpResult<()> {
    match headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        Some(origin)
            if state
                .config
                .cors
                .allow_origins
                .iter()
                .any(|allowed| allowed == origin) =>
        {
            Ok(())
        }
        Some(_) => Err(AppError::Authorization("WebSocket Origin 未获允许".into()).into()),
        None if state.config.environment.is_production() => {
            Err(AppError::Authorization("生产环境的 WebSocket 请求必须携带 Origin".into()).into())
        }
        None => Ok(()),
    }
}

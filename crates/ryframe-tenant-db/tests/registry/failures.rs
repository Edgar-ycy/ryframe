use super::super::password_tests::{PASSWORD, target};
use super::*;
use ryframe_config::{DbTlsMode, SqlLogLevel, TenantDataConfig};

#[tokio::test]
async fn failed_opening_shares_safe_cause_and_releases_reserved_budget() {
    use std::error::Error;

    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut configured = target(Some(PASSWORD));
    configured.host = Some("127.0.0.1".into());
    configured.port = Some(port);
    configured.tls_mode = Some(DbTlsMode::Disabled);
    configured.tls_ca = None;
    configured.tls_client_cert = None;
    configured.tls_client_key = None;
    let config = TenantDataConfig {
        targets: vec![configured],
        ..Default::default()
    };
    let registry = TenantDatabaseTargetRegistry::new(&config, SqlLogLevel::Off, 200).unwrap();
    let definition = registry.inner.targets.get("private-target").unwrap();
    let AcquireDecision::Wait(_opening) = registry.acquire_decision(definition).await.unwrap()
    else {
        panic!("尚未建立连接，不应返回租约");
    };
    assert_eq!(registry.pool_stats().await.opening_targets, 1);
    // accept 本身非阻塞且有截止；发送者关闭也通知退出，不留下无限等待的线程。
    listener.set_nonblocking(true).unwrap();
    let (stop, stopped) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || accept_one(listener, stopped));
    // 单线程 runtime 会先轮询全部 acquire 到同一 opening 的等待点，再推进建池任务。
    let results = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        futures_util::future::join_all((0..4).map(|_| registry.acquire("private-target"))),
    )
    .await;
    // 所有断言均在停止并回收 listener 线程之后，连 TCP 前失败或 timeout 也能收尾。
    let _ = stop.send(());
    let accepted = server.join().expect("受控 listener 线程异常");
    eprintln!("registry 真实连接失败夹具：accepted={accepted:?}; waiters={results:?}");
    assert!(
        accepted.is_ok(),
        "未证明发生真实连接：{accepted:?}; waiters={results:?}"
    );
    let results = results.expect("连接关闭后四个同轮等待者必须及时失败");
    assert_eq!(results.len(), 4);
    let mut causes = Vec::new();
    for result in results {
        let error = result.unwrap_err();
        assert_eq!(error.to_string(), "租户数据目标 private-target 暂不可用");
        causes.push(
            error
                .source()
                .expect("真实连接错误必须保留原因")
                .to_string(),
        );
        assert!(!format!("{error:?}").contains(PASSWORD));
    }
    assert!(causes.iter().all(|cause| cause == &causes[0]));
    assert!(causes[0].contains("数据库连接失败"));
    let stats = registry.pool_stats().await;
    assert_eq!(
        (
            stats.open_targets,
            stats.opening_targets,
            stats.reserved_connections
        ),
        (0, 0, 0)
    );
}

fn accept_one(
    listener: std::net::TcpListener,
    stop: std::sync::mpsc::Receiver<()>,
) -> Result<std::net::SocketAddr, String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        match listener.accept() {
            Ok((socket, peer)) => {
                // 不提供 MySQL 握手，不发送 SQL，只中断本测试实际接受的连接。
                socket
                    .shutdown(std::net::Shutdown::Both)
                    .map_err(|error| format!("已接受 {peer}，关闭连接失败：{error}"))?;
                return Ok(peer);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(format!("listener accept 失败：{error}")),
        }
        match stop.try_recv() {
            Ok(()) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                return Err("等待者已结束或控制端关闭，尚未接受 TCP 连接".into());
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
        if std::time::Instant::now() >= deadline {
            return Err("15 秒截止内未接受 TCP 连接".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn failure_tracing_uses_only_the_sanitized_cause() {
    let configured = target(Some(PASSWORD));
    let (_, definition) = super::super::support::target_definition(&configured, 1).unwrap();
    let config = DbConnection {
        password: PASSWORD.into(),
        username: "private-user".into(),
        ..Default::default()
    };
    let captured = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let subscriber = CapturedDiagnostic(captured.clone());
    tracing::subscriber::with_default(subscriber, || {
        let error = connection_failure(
            &definition,
            &config,
            "连接失败",
            &format!("1045 {PASSWORD} mysql://private-user:secret@host/db"),
        );
        assert!(!format!("{error:?}").contains(PASSWORD));
    });
    let output = captured.lock().unwrap();
    assert!(output.contains("1045"));
    let password_body = PASSWORD.trim();
    let escaped_body = format!("{password_body:?}");
    for secret in [
        PASSWORD,
        password_body,
        &escaped_body[1..escaped_body.len() - 1],
        "private-user",
        "mysql://",
        "secret@host",
    ] {
        assert!(!output.contains(secret));
    }
}

struct CapturedDiagnostic(std::sync::Arc<std::sync::Mutex<String>>);

impl tracing::Subscriber for CapturedDiagnostic {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        struct Fields<'a>(&'a mut String);
        impl tracing::field::Visit for Fields<'_> {
            fn record_debug(&mut self, _: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                use std::fmt::Write;
                write!(self.0, "{value:?}").unwrap();
            }
        }
        event.record(&mut Fields(&mut self.0.lock().unwrap()));
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

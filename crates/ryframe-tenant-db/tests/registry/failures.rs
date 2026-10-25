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
    // 仅接受同轮真实连接后关闭，不提供 MySQL 握手或执行任何 SQL。
    let server = std::thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        socket.shutdown(std::net::Shutdown::Both).unwrap();
    });
    // 单线程 runtime 会先轮询全部 acquire 到同一 opening 的等待点，再推进建池任务。
    let results = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        futures_util::future::join_all((0..4).map(|_| registry.acquire("private-target"))),
    )
    .await
    .expect("连接关闭后同轮等待者必须及时失败");
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
    server.join().unwrap();
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
    let unknown = TenantDataError::TargetUnavailable {
        target_key: "private-target".into(),
        source: None,
    };
    assert!(unknown.source().is_none());
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
    for secret in [PASSWORD, "private-user", "mysql://", "secret@host"] {
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

use crate::http_session::{
    HookFuture, NoopHook, RequestHook, RequestMetadata, Session, SessionError,
};

#[path = "http_session/authentication.rs"]
mod authentication;
#[path = "http_session/failures.rs"]
mod failures;
#[path = "http_session/fixture.rs"]
mod fixture;
#[path = "http_session/validation.rs"]
mod validation;

use fixture::{PASSWORD, Reply, Server, session};

#[derive(Default)]
struct Hook {
    before: usize,
    complete: usize,
    metadata: Vec<(String, String, String)>,
    fail_before: bool,
    fail_complete: bool,
}

impl RequestHook for Hook {
    fn before<'a>(&'a mut self, metadata: &'a RequestMetadata) -> HookFuture<'a> {
        self.before += 1;
        self.metadata.push((
            metadata.operation.clone(),
            metadata.method.clone(),
            metadata.route.clone(),
        ));
        let failed = self.fail_before;
        Box::pin(async move { hook_result(failed) })
    }

    fn complete<'a>(&'a mut self) -> HookFuture<'a> {
        self.complete += 1;
        let failed = self.fail_complete;
        Box::pin(async move { hook_result(failed) })
    }
}

fn hook_result(failed: bool) -> Result<(), SessionError> {
    if failed {
        Err(SessionError {
            category: "https://private.invalid/?password=secret",
            operation: Some(format!("https://private.invalid/?password={PASSWORD}")),
            status: Some(429),
        })
    } else {
        Ok(())
    }
}

fn run(future: impl std::future::Future<Output = ()>) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future);
}

fn assert_redacted(error: &SessionError) {
    let display = error.to_string();
    let debug = format!("{error:?}");
    for forbidden in [
        PASSWORD,
        "http://",
        "https://",
        "127.0.0.1",
        "localhost",
        "private.invalid",
        "access-secret",
        "refresh-secret",
        "error-body-secret",
    ] {
        assert!(!display.contains(forbidden), "错误显示包含敏感信息");
        assert!(!debug.contains(forbidden), "错误调试显示包含敏感信息");
    }
    assert!(std::error::Error::source(error).is_none());
}

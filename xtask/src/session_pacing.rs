//! 原生准备会话的固定节奏与共享登录预算，不启动外部脚本。

use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::Value;
use tokio::time::{Duration, Instant};

use crate::{
    http_session::{RequestHook, RequestMetadata, SessionError},
    login_budget::{Binding, LedgerFile, Reservation},
};

#[path = "session_pacing/artifact.rs"]
pub(crate) mod artifact;
#[path = "session_pacing/model.rs"]
pub(crate) mod model;

use artifact::Artifacts;
use model::{NS_PER_MS, Pacer, Summary, Wait};

pub(crate) struct PreparationHook {
    pacer: Pacer,
    ledger: LedgerFile,
    pending: Option<Vec<Reservation>>,
    artifacts: Artifacts,
    summary: Summary,
    origin: Instant,
    close_result: Option<Result<(), &'static str>>,
}

impl PreparationHook {
    pub async fn new(
        root: &Path,
        config: &Value,
        identity: &Value,
        artifacts: &Path,
        app_env: &str,
    ) -> Result<Self, SessionError> {
        let artifacts = Artifacts::new(artifacts)?;
        let started = Instant::now();
        let mut authority = None;
        let mut actual = None;
        let prepared = (|| {
            let path = model::binding(&config["bindings"]["pacing"])?;
            if !matches!(app_env, "dev" | "test") {
                return Err(error("pacing-authority"));
            }
            let config_dir = match config["bindings"].get("app_config_dir") {
                None => None,
                Some(Value::String(directory)) => Some(Path::new(directory)),
                Some(_) => return Err(error("pacing-authority")),
            };
            let loaded = crate::rate_limit::authority(root, config_dir, app_env)
                .map_err(|_| error("pacing-authority"))?;
            let loaded = serde_json::to_value(loaded).map_err(|_| error("pacing-authority"))?;
            actual = Some(artifact::digest(&loaded)?);
            authority = Some(loaded);
            let catalog = artifact::catalog(root)?;
            let authority = authority
                .as_ref()
                .ok_or_else(|| error("pacing-authority"))?;
            let pacer = Pacer::new(&config["contract"]["pacing"], authority, catalog, identity)?;
            if config["contract"]["pacing"]["authority_sha256"].as_str() != actual.as_deref() {
                return Err(error("pacing-authority-digest"));
            }
            let scope = config["bindings"]["scope_id"]
                .as_str()
                .ok_or_else(|| error("pacing-binding"))?;
            let capacity = authority["login"]["capacity"]
                .as_u64()
                .ok_or_else(|| error("pacing-authority"))?;
            let window = authority["login"]["window_ms"]
                .as_u64()
                .ok_or_else(|| error("pacing-authority"))?;
            Ok((pacer, path, scope.to_owned(), capacity, window))
        })();
        let receipt = artifact::authority_receipt(
            &prepared,
            elapsed(started)?,
            authority.as_ref(),
            actual.as_deref(),
            &config["contract"]["pacing"]["authority_sha256"],
        );
        artifacts.write("pacing-authority.json", &receipt)?;
        let (pacer, path, scope, capacity, window) = prepared?;
        let binding = Binding::new(&scope, capacity, window).map_err(|_| error("pacing-budget"))?;
        let ledger = LedgerFile::new(path, binding).map_err(|_| error("pacing-budget"))?;
        Ok(Self::from_parts(pacer, ledger, artifacts))
    }

    fn from_parts(pacer: Pacer, ledger: LedgerFile, artifacts: Artifacts) -> Self {
        Self {
            pacer,
            ledger,
            artifacts,
            pending: None,
            summary: Summary::default(),
            origin: Instant::now(),
            close_result: None,
        }
    }

    pub async fn close(&mut self) -> Result<(), SessionError> {
        if let Some(result) = self.close_result {
            return result.map_err(error);
        }
        // 写入开始前即锁定失败状态，未知结果不能通过重试覆盖。
        self.close_result = Some(Err("pacing-close"));
        let completed = self.complete_request();
        let written = self.artifacts.write(
            "pacing-summary.json",
            &self.summary.document(self.pacer.pacing()),
        );
        let result = match (completed, written) {
            (Ok(()), result) | (result, Ok(())) => result,
            (Err(_), Err(_)) => Err(error("pacing-close-complete-and-artifact")),
        };
        self.close_result = Some(result.as_ref().map(|_| ()).map_err(|error| error.category));
        result
    }

    async fn before_request(&mut self, metadata: &RequestMetadata) -> Result<(), SessionError> {
        if self.close_result.is_some() {
            return Err(error("pacing-closed"));
        }
        if self.pending.is_some() {
            return Err(error("pacing-pending"));
        }
        let before = elapsed(self.origin)?;
        let plan = self.pacer.plan(metadata, before)?;
        wait(self.origin, before, plan.wait_ns).await?;
        let finished = elapsed(self.origin)?;
        if metadata.operation == "post_auth_login" {
            self.reserve().await?;
        }
        let started = elapsed(self.origin)?;
        self.pacer.sent(&plan, before, finished, started)?;
        self.summary
            .fixed(&metadata.operation, plan.wait_ns, finished - before);
        Ok(())
    }

    async fn reserve(&mut self) -> Result<(), SessionError> {
        let began = elapsed(self.origin)?;
        let deadline = model::budget_deadline(began, self.pacer.login_window())?;
        loop {
            if elapsed(self.origin)? > deadline {
                return Err(error("pacing-budget-timeout"));
            }
            let identity = self.pacer.identity();
            let result = self
                .ledger
                .try_reserve(
                    &identity.tenant,
                    &identity.username,
                    &identity.address,
                    wall_time()?,
                )
                .map_err(|_| error("pacing-budget"))?;
            if let Some(reservation) = result.reservation {
                self.pending = Some(reservation);
                return Ok(());
            }
            let before = elapsed(self.origin)?;
            model::budget_wait(before, result.wait_ms, deadline)?;
            self.summary.budget_wait(result.wait_ms);
            let requested = result
                .wait_ms
                .checked_mul(NS_PER_MS)
                .ok_or_else(|| error("pacing-clock"))?;
            wait(self.origin, before, requested).await?;
            self.summary.budget_duration(elapsed(self.origin)? - before);
        }
    }

    fn complete_request(&mut self) -> Result<(), SessionError> {
        // 先取走预约，即使账本完成失败也不再次提交未知结果；没有退款路径。
        if let Some(reservation) = self.pending.take() {
            self.ledger
                .complete(&reservation, wall_time()?)
                .map_err(|_| error("pacing-budget-complete"))?;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl RequestHook for PreparationHook {
    async fn before(&mut self, metadata: &RequestMetadata) -> Result<(), SessionError> {
        self.before_request(metadata).await
    }

    async fn complete(&mut self) -> Result<(), SessionError> {
        self.complete_request()
    }
}

async fn wait(origin: Instant, before: u64, requested: u64) -> Result<(), SessionError> {
    let mut waiting = Wait::new(before, requested)?;
    while let Some(remaining) = waiting.remaining(elapsed(origin)?)? {
        tokio::time::sleep(Duration::from_millis(remaining.div_ceil(NS_PER_MS))).await;
    }
    Ok(())
}

fn elapsed(origin: Instant) -> Result<u64, SessionError> {
    u64::try_from(
        Instant::now()
            .checked_duration_since(origin)
            .ok_or_else(|| error("pacing-clock"))?
            .as_nanos(),
    )
    .map_err(|_| error("pacing-clock"))
}

fn wall_time() -> Result<u64, SessionError> {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| error("pacing-clock"))?
            .as_millis(),
    )
    .map_err(|_| error("pacing-clock"))
}

pub(crate) fn error(category: &'static str) -> SessionError {
    SessionError {
        category,
        operation: None,
        status: None,
    }
}

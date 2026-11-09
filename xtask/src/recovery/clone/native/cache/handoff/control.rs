//! API 只消费一次原生启动预留；收取失败或晚到结果不触发重放与追加停止。
use super::{
    diagnostic::{self, Phase},
    input::Frame,
    locks,
};
use crate::{
    Result,
    recovery::{
        clone::native::{bridge::scope::Scope, target_verify::guard::Generation},
        full_stack::worker::{self, role::Role},
    },
};
use serde_json::{Value, json};

pub(super) fn start(
    frame: &Frame,
    owner: &Generation,
    scope: &Scope,
    storage: &std::path::Path,
) -> Result<Value> {
    let lock = diagnostic::context(
        &frame.run,
        Some(frame.number),
        Phase::BeforeStart,
        locks::Runtime::acquire(&frame.backend, &frame.post.runtime),
    )?;
    let result = (|| {
        let (reservation, token) = diagnostic::context(
            &frame.run,
            Some(frame.number),
            Phase::BeforeStart,
            (|| {
                super::guard::before(frame, owner, scope, storage)?;
                lock.check(&frame.backend)?;
                let reservation = worker::start::reserve(
                    Role::Api,
                    &frame.product,
                    &frame.post.runtime,
                    &frame.post.request["api_environment"],
                )?;
                let token = reservation.id().to_owned();
                super::history::intent(frame, &token)?;
                frame.check(owner, scope)?;
                lock.check(&frame.backend)?;
                Ok((reservation, token))
            })(),
        )?;
        let started = diagnostic::context(
            &frame.run,
            Some(frame.number),
            Phase::Start,
            reservation.start(60.0),
        )?;
        diagnostic::context(
            &frame.run,
            Some(frame.number),
            Phase::AfterStart,
            (|| {
                super::history::started(frame, &token, &started["identity"])?;
                // 原生控制器已经核对同一操作的完整树、监听和结果；仍须完成原交接全部后守卫。
                super::guard::after(frame, owner, scope, storage)?;
                lock.check(&frame.backend)?;
                Ok(
                    json!({"format_version":1,"kind":"devex-clone-runtime-control","operation":"start",
            "scope_id":frame.runtime["scope_id"],"runtime_directory":frame.post.runtime,
            "processes":{"api":{"state":"running","identity":started["identity"],"ready":true,
                "log":format!("api-{token}.log")}},
            "producer_history":frame.post.runtime.join("producer-history.json")}),
                )
            })(),
        )
    })();
    diagnostic::context(
        &frame.run,
        Some(frame.number),
        Phase::Cleanup,
        lock.finish(&frame.backend, result),
    )
}

//! 正式来源验证父执行链；远端未知结果只保留证据，不重放生产者或缓存事务。
use super::session::Session;
use crate::{
    Result,
    recovery::clone::native::{
        export::transport::{SystemTransport, Transport},
        json as document,
    },
};
use serde_json::{Value, json};
use std::path::Path;

#[path = "execution/normal.rs"]
mod normal;
#[path = "execution/recover.rs"]
mod recover;
#[path = "execution/saved.rs"]
pub(super) mod saved;
#[path = "execution/warm.rs"]
mod warm;

pub(in crate::recovery::source) fn run(
    backend: &Path,
    start: &Path,
    output: &Path,
    recover: bool,
) -> Result<Value> {
    run_with(backend, start, output, recover, &SystemTransport)
}

// 正式入口与验收仅选择资源端口；来源、锁、阶段与发布仍由同一 Session 执行。
pub(super) fn run_with<T: Transport + Sync>(
    backend: &Path,
    start: &Path,
    output: &Path,
    recover: bool,
    transport: &T,
) -> Result<Value> {
    let session = Session::open(backend, start, output, recover, transport)?;
    let result = if recover {
        recover::run(&session)
    } else {
        normal::run(&session)
    };
    let result = match result {
        Err(error) if !recover && session.directory.is_dir() => {
            let failed = session.directory.join("failed.json");
            let published = document::write_new(
                &failed,
                &json!({"status":"failed","error":"SourceRuntimeError"}),
            );
            match published {
                Ok(()) => Err(error),
                Err(publication) => Err(format!("来源验证失败：{error}；失败收据 {} 发布也失败：{publication}；保留现场，禁止重放",failed.display()).into()),
            }
        }
        result => result,
    };
    session.finish(result)
}

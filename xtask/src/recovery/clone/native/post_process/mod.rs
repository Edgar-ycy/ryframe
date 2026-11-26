//! 复制后处理生产者的计划、证据核验、精确回收和私有协议。

pub(crate) mod amend;
pub(in crate::recovery::clone::native) mod api_successor;
mod business_binding;
pub(crate) mod command;
mod context;
pub(crate) mod diagnostic;
pub(crate) mod entry;
mod execution;
pub(super) mod files;
mod images;
mod inspection;
mod live;
mod lock;
pub(super) mod ownership;
mod prepare;
mod producer;
pub(crate) mod protocol;
pub(super) mod recover;
pub(crate) mod registration;
pub(crate) mod runtime;
mod schedule_images;
pub(in crate::recovery::clone::native) use schedule_images::read_target_control;
mod schedules;
pub(super) mod seed_capacity;
mod seed_close;
mod seed_department;
mod seed_identity;
mod seed_identity_reconcile;
mod seed_prepare;
mod seed_start;
#[cfg(windows)]
pub(super) mod source_proofs;
pub(super) use inspection::inspect_historical;
pub(super) use prepare::require_runtime_receipts;

pub(crate) fn require_quiet(
    backend: &std::path::Path,
    run: &std::path::Path,
    current: Option<u64>,
) -> crate::Result<()> {
    use super::{json as document, lifecycle::state, resources::common, seed};
    let saved = state::load(run, false)?;
    let mut registrations = serde_json::Map::new();
    for attempt in state::attempts(&saved)? {
        let number = attempt["number"].as_u64().ok_or("生产者阶段序号无效")?;
        if current == Some(number) {
            continue;
        }
        if inspection::unpublished_seed_registration(backend, run, attempt)? {
            continue;
        }
        let stage = document::text(&attempt["stage"], "生产者阶段")?;
        let pair = match stage {
            "post-copy" => {
                let post = seed::history::post(backend, run, None, None, true)?;
                (post.request, post.descriptor)
            }
            "seed-runtime" => {
                let descriptor = common::binding(&run.join("seed-runtime.json"))?;
                (
                    registration::resolve(backend, run, stage, &descriptor, None, true)?,
                    descriptor,
                )
            }
            _ => continue,
        };
        registrations.insert(
            number.to_string(),
            serde_json::json!({"request":pair.0,"binding":pair.1}),
        );
    }
    inspection::quiet(&serde_json::json!({"backend":backend,"run_dir":run,
        "current_attempt":current,"registrations":registrations}))
    .map(|_| ())
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/post_process/tests.rs"]
mod tests;

pub(crate) fn runtime_proof(request: &serde_json::Value) -> crate::Result<serde_json::Value> {
    runtime::proof(request)
}

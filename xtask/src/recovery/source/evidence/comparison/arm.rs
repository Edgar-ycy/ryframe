//! ARM 复用正式来源账本、已发布共享导出和静止代次。
use std::path::Path;

use serde_json::{Value, json};

use super::{common, document, generation, path, published};
use crate::{
    Result,
    recovery::{
        clone::native::{
            cache,
            lifecycle::state,
            seed::{arm_input::history::Active, protocol::guards::rebind::live},
            target_verify::{authority, published as target},
        },
        source::evidence::running,
    },
};

#[derive(PartialEq)]
pub(in crate::recovery) struct Source {
    pub(in crate::recovery) value: Value,
    state: Value,
}

pub(in crate::recovery) fn load(
    backend: &Path,
    active: &Active<'_>,
    successor: bool,
    live_storage: bool,
) -> Result<Source> {
    let request = active.request;
    let (original, receipt) = if successor {
        let published = published::verified_for_arm(
            backend,
            &request["source_export_result"],
            Some(&request["review_successor"]),
            Some(active),
        )?;
        for (source, requested) in [
            ("source_registration", "source_registration"),
            ("source_rebind", "source_rebind"),
            ("source_generation", "source_generation"),
            ("export", "source_export"),
        ] {
            if published.identity[source].is_null()
                || published.identity[source] != request[requested]
            {
                return Err("successor arm 未绑定同一已发布共享导出或来源代次".into());
            }
        }
        if published.source["directory"] != json!(active.run)
            || published.source["registration"]["run_directory"] != json!(active.run)
        {
            return Err("successor arm 的 C52 来源不属于当前持锁 run".into());
        }
        let original = if live_storage {
            cache::binding::source::load(backend, &request["review_successor"])?
        } else {
            published.source.clone()
        };
        (original, Some(published))
    } else {
        let source = cache::binding::source::published(backend, &request["source_registration"])?;
        target::request_binding(backend, &source["seed_target"])?;
        (source, None)
    };
    resolve(backend, active, original, receipt, live_storage)
}

fn resolve(
    backend: &Path,
    active: &Active<'_>,
    original: Value,
    receipt: Option<published::Verified>,
    live_storage: bool,
) -> Result<Source> {
    let request = active.request;
    let run = path(&original["directory"])?;
    let descriptor = common::binding(&run.join("state.json"))?;
    let saved = state::load(&run, true)?;
    let source = if let Some(receipt) = &receipt {
        if live_storage {
            let live = running::history::resolve_published_storage(
                backend,
                &run,
                &saved,
                &request["source_registration"],
                original.clone(),
                true,
                Some(active),
            )?;
            for field in [
                "storage",
                "source_rebind",
                "registration",
                "review_successor_binding",
            ] {
                if live[field] != receipt.source[field] {
                    return Err("arm 实际来源存储或重绑定不属于已发布共享导出".into());
                }
            }
        }
        receipt.source.clone()
    } else {
        let rebound = running::history::resolve_published_storage(
            backend,
            &run,
            &saved,
            &request["source_registration"],
            original.clone(),
            live_storage,
            Some(active),
        )?;
        generation::resolve(backend, &rebound, &saved)?
    };
    if source["seed_target"]["side"] != "seed"
        || request["source_rebind"] != source["source_rebind"]
        || request.get("source_rebind").is_some_and(Value::is_null)
        || receipt
            .as_ref()
            .is_some_and(|receipt| receipt.state != descriptor)
    {
        return Err("arm 请求未绑定当前已发布 seed 和显式重绑定".into());
    }
    if live_storage {
        require_quiet(backend, &original, &source)?;
    }
    if let Some(receipt) = &receipt {
        receipt.check(backend)?;
    }
    common::bound_file(backend, &descriptor, "arm 来源原账本")?;
    Ok(Source {
        value: source,
        state: descriptor,
    })
}

fn require_quiet(backend: &Path, original: &Value, current: &Value) -> Result<()> {
    live::quiet(
        backend,
        &json!({"registration":original["registration"],"request":original["request"]}),
    )?;
    if !current["source_generation"].is_null() {
        let request = &current["request"];
        let runtime = common::read_bound(backend, &request["runtime"], "arm 后继源运行")?;
        authority::require_closed(document::text(
            &request["source"]["api_url"],
            "arm 来源 API",
        )?)?;
        authority::require_closed(document::text(
            &runtime["worker_ready_url"],
            "arm 来源 Worker",
        )?)?;
    }
    Ok(())
}

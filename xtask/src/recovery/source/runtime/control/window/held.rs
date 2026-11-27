//! STOP 仅在完整像采集区间持有原静态构建，保留实时角色和储存观察。
use serde_json::Value;

use super::{Window, common, document, running};
use crate::{
    Result,
    recovery::{
        clone::native::{export::transport::Transport, target_verify::authority::require_closed},
        source::evidence::running::builds::held::HeldBuilds,
    },
};

pub(super) fn capture<T: Transport + Sync>(
    window: &Window<'_, '_, T>,
    start: &Value,
    phase: &str,
) -> Result<(Value, Value)> {
    let backend = window.frame.backend;
    let request = &window.facts["request"];
    let opened = (|| {
        window.checkpoint()?;
        let build = common::read_bound(backend, &request["backend_build"], "来源原产品构建")?;
        HeldBuilds::open(backend, request, &build)
    })();
    let held = match opened {
        Ok(held) => held,
        Err(error) => return combine(Err(error), window.checkpoint()),
    };
    let live = phase == "stop-before";
    let checkpoint = || {
        window.checkpoint_with(|| {
            running::load_held(
                backend,
                start,
                running::HeldLoad::Historical { live },
                &held,
            )
        })?;
        if !live {
            let api = document::text(&window.facts["selected"]["api_url"], "源 API 地址")?;
            let worker = document::text(
                &window.facts["runtime"]["worker_ready_url"],
                "源 Worker 地址",
            )?;
            combine(require_closed(api), require_closed(worker))?;
        }
        Ok(())
    };
    let result = checkpoint().and_then(|()| {
        let output = window.output()?.join(phase);
        window.capture_with(&output, &checkpoint)
    });
    let result = combine(result, held.finish(backend, request));
    let result = combine(result, window.checkpoint());
    combine(result, checkpoint())
}

fn combine<T>(result: Result<T>, checked: Result<()>) -> Result<T> {
    match (result, checked) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(checked)) => Err(format!(
            "源 STOP 采集失败：{error}；保护窗口复核也失败：{checked}；保留现场，禁止重放"
        )
        .into()),
    }
}

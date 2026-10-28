//! 保留旧正式 caller 的生产者静默门禁；复用原有固定原生工具及协议。
use super::Controller;
use crate::{
    Result,
    recovery::clone::native::{
        json as document,
        lifecycle::state,
        resources::common,
        seed::{Registration, history},
    },
};
use serde_json::json;
use std::time::Duration;

pub(super) fn require(controller: &Controller<'_>) -> Result<()> {
    let backend = controller.backend();
    let run = controller.run();
    let mut registrations = serde_json::Map::new();
    for attempt in state::attempts(controller.saved())? {
        if attempt["number"].as_u64() == Some(controller.number()) {
            continue;
        }
        let registration = match attempt["stage"].as_str() {
            Some("post-copy") => {
                let post = history::post(backend, run, None, None, true)?;
                json!({"request":post.request,"binding":post.descriptor})
            }
            Some("seed-runtime") => {
                let seed = Registration::load(backend, run, true)?;
                json!({"request":seed.value,"binding":common::binding(&run.join("seed-runtime.json"))?})
            }
            _ => continue,
        };
        let number = attempt["number"].as_u64().ok_or("生产者历史序号无效")?;
        registrations.insert(number.to_string(), registration);
    }
    let input = serde_json::to_vec(&json!({"operation":"quiet","payload":{
        "backend":backend,"run_dir":run,"current_attempt":controller.number(),"registrations":registrations}}))?;
    if input.len() > 1024 * 1024 {
        return Err("生产者静默原生请求超过既有 1 MiB 合同".into());
    }
    let output = crate::process::command_output_with_input_timeout(
        backend,
        document::text(&controller.tool()["path"], "固定重绑定原生工具")?,
        &["__clone-post-process"],
        &[],
        Some(&input),
        Duration::from_secs(60),
    )?;
    if !output.status.success() {
        return Err("历史生产者静默协议失败；须核对登记、创建身份和未收尾现场".into());
    }
    let text = std::str::from_utf8(&output.stdout).map_err(|_| "生产者静默响应不是 UTF-8")?;
    let response = document::parse_fragment(text, "生产者静默响应")?;
    document::exact(&response, &["ok", "result"], "生产者静默响应")?;
    if response["ok"] != true || response["result"] != json!({"quiet":true}) {
        return Err("生产者静默未返回确切成功证明".into());
    }
    Ok(())
}

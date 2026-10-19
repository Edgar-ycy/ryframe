use serde_json::{Value, json};

use super::{TargetContext, common};
use crate::{Result, cli::CloneSide};

pub(super) struct Selected {
    pub(super) storage: Value,
    pub(super) runtime: Option<Value>,
}

pub(super) fn select(context: &TargetContext) -> Result<Selected> {
    let expected = &context.initialized["generation"]["storage"];
    let mut selected = Selected {
        storage: expected.clone(),
        runtime: None,
    };
    if let Some(run) = super::super::super::lifecycle::manifest::target_storage_path(
        &context.backend,
        &context.manifest,
    )? {
        // 没有可信账本来源时，不能把内嵌缓存值当作授权。
        if context.manifest["target_storage_run"]
            .get("cache")
            .is_some_and(|value| !value.is_null())
        {
            return Err("目标缓存重启代次缺少原生可信账本解析，拒绝采用内嵌缓存绑定".into());
        }
        // 此处只派生可信账本；实际 PID、argv 与监听事实统一由 StorageObserver 核验。
        let runtime = super::super::super::resources::storage::registered::registered(
            &context.backend,
            &run,
            CloneSide::Target,
            None,
        )?;
        if runtime.as_ref()
            != context.manifest["target_storage_run"]
                .get("storage")
                .filter(|value| !value.is_null())
        {
            return Err("目标当前存储代次与清单登记不同".into());
        }
        if let Some(runtime) = runtime {
            let request =
                common::read_bound(&context.backend, &runtime["request"], "存储重启请求")?;
            for key in ["identity", "process_receipt", "launch_receipt"] {
                if request["previous"].get(key) != context.request["storage"]["rustfs"].get(key) {
                    return Err("存储重启未绑定原目标完整进程收据".into());
                }
            }
            selected.storage["rustfs"] = runtime["storage"].clone();
            selected.runtime = Some(runtime);
        }
    }
    validate_transition(expected, &selected.storage, selected.runtime.as_ref(), None)?;
    Ok(selected)
}

pub(crate) fn validate_transition(
    expected: &Value,
    actual: &Value,
    runtime: Option<&Value>,
    cache: Option<&Value>,
) -> Result<()> {
    if actual == expected {
        return Ok(());
    }
    let allowed = json!({
        "rustfs": runtime.map(|value| &value["storage"]).unwrap_or(&expected["rustfs"]),
        "redis": cache.map(|value| &value["redis"]).unwrap_or(&expected["redis"]),
    });
    if actual != &allowed {
        return Err("目标存储变化超出已登记的完整重启代次".into());
    }
    Ok(())
}

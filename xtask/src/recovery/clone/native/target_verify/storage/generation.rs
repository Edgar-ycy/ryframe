use serde_json::{Value, json};

use super::{TargetContext, arm_fixture_projection};
use crate::{
    Result,
    recovery::clone::native::lifecycle::manifest::{self, fixture_storage},
};

pub(in crate::recovery::clone::native::target_verify) struct Selected {
    pub(in crate::recovery::clone::native::target_verify) storage: Value,
    pub(super) runtime: Option<Value>,
    fixture: Option<fixture_storage::Fixture>,
}

impl Selected {
    pub(in crate::recovery::clone::native::target_verify) fn check(
        &self,
        context: &TargetContext,
    ) -> Result<()> {
        match &self.fixture {
            Some(fixture) => fixture.check(&context.backend),
            None => Ok(()),
        }
    }
}

pub(in crate::recovery::clone::native::target_verify) fn select(
    context: &TargetContext,
    owner: &super::super::guard::Generation,
) -> Result<Selected> {
    let expected = &context.initialized["generation"]["storage"];
    if context.manifest["copy_stage"] != "seed_to_arm" {
        return Ok(Selected {
            storage: expected.clone(),
            runtime: None,
            fixture: None,
        });
    }
    let descriptor = &context.manifest["target_storage_run"];
    if let Some(fixture) = fixture_storage::resolve(
        &context.backend,
        &context.manifest["target_registration"],
        Some(descriptor),
        true,
    )? {
        let (selected, _) = arm_fixture_projection(&context.backend, &fixture.run, expected)?;
        fixture.check(&context.backend)?;
        return Ok(Selected {
            storage: selected.value,
            runtime: selected.runtime,
            fixture: Some(fixture),
        });
    }
    let run = manifest::target_storage_path(&context.backend, &context.manifest)?
        .ok_or("seed_to_arm 缺少有效的独立目标存储目录")?;
    let (selected, cache) = super::super::fresh::selection::transition(
        &context.backend,
        &context.request,
        Some(&run),
        Some(owner),
    )?;
    if selected.runtime.as_ref() != Some(&descriptor["storage"])
        || cache.as_ref() != Some(&descriptor["cache"])
    {
        return Err("目标当前存储代次与清单登记不同".into());
    }
    validate_transition(
        expected,
        &selected.value,
        selected.runtime.as_ref(),
        cache.as_ref(),
    )?;
    Ok(Selected {
        storage: selected.value,
        runtime: selected.runtime,
        fixture: None,
    })
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

/// 外层配置和来源必须逐字段保持原像；只有完整 storage 可按已登记代次替换。
pub(crate) fn validate_complete(
    expected: &Value,
    actual: &Value,
    runtime: Option<&Value>,
    cache: Option<&Value>,
) -> Result<()> {
    let mut outer = super::json_io::object(expected, "原目标完整代次")?.clone();
    let observed = super::json_io::object(actual, "当前目标完整代次")?;
    if !outer.contains_key("storage") || !observed.contains_key("storage") {
        return Err("目标完整代次缺少 storage".into());
    }
    super::json_io::exact(&expected["storage"], &["rustfs", "redis"], "原存储代次")?;
    super::json_io::exact(&actual["storage"], &["rustfs", "redis"], "当前存储代次")?;
    validate_transition(&expected["storage"], &actual["storage"], runtime, cache)?;
    outer.insert("storage".into(), actual["storage"].clone());
    if &outer != observed {
        return Err("已登记存储恢复之外的目标源码、配置或资源代次发生变化".into());
    }
    Ok(())
}

//! 所有失败记录必须由既有完整续作确认解除；异常始终未解决。
use std::path::Path;

use super::super::{
    super::json as json_io, evidence_model::*, evidence_prepare, guard::Generation, prepare_state,
    resume, resume_confirmed, resume_model,
};
use crate::Result;

pub(super) fn resolved(backend: &Path, target: &Path, owner: Option<&Generation>) -> Result<bool> {
    if let Some(owner) = owner {
        owner.protects(target)?;
    }
    let qualified = if names(target, r"^failure.*\.json$")?.is_empty() {
        None
    } else {
        Some(qualification(backend, target, owner).unwrap_or(false))
    };
    if let Some(owner) = owner {
        owner.protects(target)?;
    }
    let Some(qualified) = qualified else {
        return Ok(false);
    };
    check(qualified, "初始化历史存在未解决的失败记录")?;
    Ok(target.join("failure.json").is_file())
}

fn qualification(backend: &Path, target: &Path, owner: Option<&Generation>) -> Result<bool> {
    let prepared = read(backend, &target.join("prepare.json"))?;
    json_io::exact(&prepared, resume_model::PREPARE, "prepare")?;
    let descriptor = &prepared["request"];
    let active = bound(
        backend,
        &evidence_prepare::failure_path(backend, target, descriptor)?,
    )?;
    if target.join("initialized.json").is_file() {
        let initialized = read(backend, &target.join("initialized.json"))?;
        let inventory = resume::inventory::confirmed(backend, target, &initialized, owner);
        let migration = resume_confirmed::confirmed(backend, target, &initialized, owner);
        if inventory || migration {
            return Ok(true);
        }
    }
    Ok(active == target.join("failure.json")
        && prepare_state::resolved(backend, target, descriptor))
}

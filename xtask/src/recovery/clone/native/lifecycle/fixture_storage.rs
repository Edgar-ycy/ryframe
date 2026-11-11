//! 夹具 arm 只冻结原服务账本可证明的首代或后继代次。
use super::{bound_file, exact, json_io, text};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::{clone::native::lifecycle::state, fixture_control::services::history},
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(crate) struct Fixture {
    pub(in crate::recovery) run: PathBuf,
    pub(crate) binding: Value,
    current_state: Value,
    registration: Value,
}

impl Fixture {
    pub(in crate::recovery) fn state_binding(&self) -> &Value {
        &self.current_state
    }

    pub(crate) fn check(&self, backend: &Path) -> Result<()> {
        bound_file(backend, &self.registration, "arm fresh 原登记", true)?;
        bound_file(backend, &self.binding["manifest"], "arm 夹具清单", true)?;
        if bound_file(backend, &self.current_state, "arm 夹具账本", true)?
            != self.run.join("state.json")
        {
            return Err("arm 夹具账本不属于原运行目录".into());
        }
        Ok(())
    }
}

/// 以原 RustFS 进程收据定位夹具，已移交时只允许接续指定复制账本。
pub(in crate::recovery) fn rustfs_handoff(
    backend: &Path,
    copy: &Path,
    request: &Value,
) -> Result<Option<Value>> {
    use crate::recovery::clone::native::resources::common;

    let process = &request["previous"]["process_receipt"];
    let path = bound_file(backend, process, "存储重启原进程收据", true)?;
    let Some(service) = path
        .parent()
        .filter(|path| path.ends_with("rustfs"))
        .and_then(Path::parent)
    else {
        return Ok(None);
    };
    let manifest_path = service.join("manifest.json");
    let Some(manifest) =
        crate::recovery::clone::native::resources::origin::receipt_file(&manifest_path)?
    else {
        return Ok(None);
    };
    if manifest["kind"] != "reference-fixture-service-run" {
        return Ok(None);
    }
    history::validate_manifest(&manifest)?;
    let current = common::binding(&service.join("state.json"))?;
    let value = state::load(service, true)?;
    let fixture = history::validate(service, &value, true, true)?;
    let descriptor = &fixture["rustfs_handoff"];
    if descriptor.is_null() {
        return Ok(None);
    }
    let handoff = common::read_bound(backend, descriptor, "RustFS 移交结果")?;
    if Path::new(text(&handoff["copy_state"]["path"], "移交复制账本")?) != copy.join("state.json")
    {
        return Err("RustFS 已移交其他复制账本".into());
    }
    let observed = state::historical(copy, &handoff["copy_state"], true)?;
    let proof = common::read_bound(
        backend,
        &handoff["evidence"]["rustfs"],
        "RustFS 移交整树收据",
    )?;
    let last = observed["historical_attempts"]
        .as_u64()
        .and_then(|count| count.checked_sub(1))
        .and_then(|index| usize::try_from(index).ok())
        .and_then(|index| observed["state"]["attempts"].get(index))
        .ok_or("RustFS 移交的复制账本前缀无关闭结果")?;
    if fixture["closed"] != false
        || value["manifest"] != common::binding(&manifest_path)?
        || last["result"] != handoff["seed_close"]
        || proof["tree"]["process"] != request["previous"]["identity"]
        || proof["data_directory"] != request["data_directory"]
        || common::binding(&service.join("state.json"))? != current
    {
        return Err("RustFS 重启的原代次、数据目录或移交账本不一致".into());
    }
    Ok(Some(descriptor.clone()))
}

/// None 只表示摘要已核定的清单不是夹具服务清单。
pub(crate) fn resolve(
    backend: &Path,
    registration: &Value,
    effective: Option<&Value>,
    active: bool,
) -> Result<Option<Fixture>> {
    let registration_path = bound_file(backend, registration, "arm fresh 原登记", true)?;
    let fresh = json_io::read(&registration_path)?;
    let historical = exact(
        &fresh["storage_run"],
        &["path", "manifest", "state"],
        "历史 target_storage_run",
    )?;
    let run = PathBuf::from(text(&historical["path"], "arm 目标存储路径")?);
    validate_local_test_path(&run, backend, LocalTestPathKind::ExistingDirectory)?;
    let manifest_path = bound_file(backend, &historical["manifest"], "arm 目标存储清单", true)?;
    if manifest_path != run.join("manifest.json") {
        return Err("arm 目标存储清单不属于原运行目录".into());
    }
    let manifest = json_io::read(&manifest_path)?;
    if manifest["kind"] != "reference-fixture-service-run" {
        bound_file(backend, registration, "arm fresh 原登记", true)?;
        bound_file(backend, &historical["manifest"], "arm 目标存储清单", true)?;
        return Ok(None);
    }
    exact(
        &fresh,
        &[
            "format_version",
            "kind",
            "request",
            "environment",
            "storage_run",
            "target_directory",
        ],
        "arm fresh 原登记",
    )?;
    if fresh["format_version"] != 1
        || fresh["kind"] != "devex-clone-fresh-target-registration"
        || registration_path
            .file_name()
            .is_none_or(|name| name != "registration.json")
        || registration_path
            .parent()
            .map(|parent| parent.join("target"))
            .as_deref()
            != Some(Path::new(text(
                &fresh["target_directory"],
                "arm fresh 目标目录",
            )?))
    {
        return Err("arm 夹具代次必须绑定原 fresh 目标登记".into());
    }
    history::validate_manifest(&manifest)?;
    let observed = state::historical(&run, &historical["state"], true)?;
    let binding = select(
        &run,
        &historical["path"],
        &historical["manifest"],
        &observed,
        effective,
        active,
    )?;
    let fixture = Fixture {
        run,
        binding,
        current_state: observed["current"].clone(),
        registration: registration.clone(),
    };
    fixture.check(backend)?;
    Ok(Some(fixture))
}

fn select(
    run: &Path,
    path: &Value,
    manifest: &Value,
    observed: &Value,
    effective: Option<&Value>,
    active: bool,
) -> Result<Value> {
    let state = &observed["state"];
    if state["manifest"] != *manifest {
        return Err("arm 夹具账本与原清单不同".into());
    }
    let count = observed["historical_attempts"]
        .as_u64()
        .and_then(|count| usize::try_from(count).ok())
        .ok_or("arm 夹具历史长度无效")?;
    let tail = state::attempts(state)?
        .get(count..)
        .ok_or("arm 夹具历史越界")?;
    let current = history::validate(run, state, true, true)?;
    let registered = if tail.is_empty() {
        current["active_generation"].clone()
    } else {
        let mut prefix = state.clone();
        prefix["attempts"]
            .as_array_mut()
            .ok_or("arm 夹具历史无阶段列表")?
            .truncate(count);
        history::validate(run, &prefix, true, true)?["active_generation"].clone()
    };
    let generation = if let Some(effective) = effective {
        let fields = exact(
            effective,
            &["path", "manifest", "generation"],
            "arm 夹具有效代次",
        )?;
        if fields["path"] != *path || fields["manifest"] != *manifest {
            return Err("arm 夹具有效代次不属于原运行目录".into());
        }
        let generation = &fields["generation"];
        if generation != &registered
            && !tail.iter().any(|attempt| {
                attempt["stage"] == "fixture-services"
                    && attempt["mode"] == "restart"
                    && attempt["status"] == "passed"
                    && attempt["result"] == *generation
            })
        {
            return Err("arm 夹具代次不是登记首选代次或合法后继".into());
        }
        generation
    } else {
        &current["active_generation"]
    };
    if (active || effective.is_none())
        && (current["closed"] != false
            || !current["external_recovery"].is_null()
            || !current["rustfs_handoff"].is_null()
            || generation != &current["active_generation"])
    {
        return Err("arm 夹具当前代次不可用或与原有效代次不同".into());
    }
    Ok(json!({"path":path,"manifest":manifest,"generation":generation}))
}

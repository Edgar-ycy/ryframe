use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Map, Value, json};

use super::{
    arm,
    document::object,
    domain::{identifier, validate_independent_b1_plan, validate_plan},
    reference_configuration::{SourceProjection, TargetProjection},
    target::TargetRequest,
};
use crate::{
    Result,
    cli::{ReferenceInputMode, ReferenceInputOptions, RestoreInputSide},
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::{
        fixture_control::successor::arm_input::IndependentB1Receipt,
        runtime::dataset_authority::reads::Window,
    },
};

pub(super) const INDEPENDENT_B1_KIND: &str = "restore-reference-independent-b1-plan";

pub(crate) fn source_projection(backend: &Path, descriptor: &Value) -> Result<Value> {
    let request = super::source::load(backend, descriptor)?;
    let projection = super::reference_configuration::SourceProjection::load(backend, &request)?;
    projection.assert_unchanged(backend, &request)?;
    Ok(json!({"source": projection.source, "tools": projection.tools}))
}

pub(crate) fn build(backend: &Path, options: &ReferenceInputOptions) -> Result<Value> {
    identifier(&options.id)?;
    if let Some(publication) = &options.publication {
        validate_publication_path(&options.work_dir, &publication.output)?;
    }
    match &options.mode {
        ReferenceInputMode::Successor {
            fresh_target_verify,
        } => build_successor(backend, options, fresh_target_verify),
        ReferenceInputMode::IndependentB1 => build_independent_b1(backend, options),
    }
}

fn build_successor(backend: &Path, options: &ReferenceInputOptions, fresh: &Path) -> Result<Value> {
    let (arm, _fresh) = arm::load(backend, &options.arm_input, fresh, options.side.as_str())?;
    let source_request = object(&arm.source_request.value, "共享源导出请求")?;
    let selected = object(&arm.target.selected, "审阅单侧资源")?;
    let configuration = super::reference_configuration::Projection::load(backend, &arm)?;
    let directory = new_path(backend, &options.work_dir, "参考计划 work_dir")?;
    independent_work_dir(&directory, source_request, selected, &arm, fresh)?;
    let plan = project(
        &options.id,
        options.side,
        &directory,
        &arm.target,
        &configuration.source,
        &configuration.target,
    )?;
    validate_plan(&plan, backend)?;
    configuration.assert_unchanged(backend, &arm)?;
    arm.target.document.assert_unchanged()?;
    arm.result.assert_unchanged()?;
    arm.request.assert_unchanged()?;
    arm.manifest.assert_unchanged()?;
    arm.source_request.assert_unchanged()?;
    Ok(plan)
}

fn build_independent_b1(backend: &Path, options: &ReferenceInputOptions) -> Result<Value> {
    if options.side != RestoreInputSide::Candidate {
        return Err("独立 B1 参考计划只允许 candidate 侧".into());
    }
    let window = Window::direct()?;
    let result = (|| {
        let arm = IndependentB1Receipt::read(backend, &options.arm_input)?;
        if let Some(publication) = &options.publication {
            // 通用发布器会在写后重建计划，此时输出已存在。
            arm.require_disjoint(&publication.output)?;
        }
        let reference = project_independent_b1(backend, &options.id, &options.work_dir, &arm)?;
        arm.check()?;
        Ok(json!({
            "format_version":1, "kind":INDEPENDENT_B1_KIND,
            "reference_plan":reference,
            "arm_input":arm.result_binding(), "source_export":arm.source_export(),
        }))
    })();
    window.finish(result)
}

pub(super) fn project_independent_b1(
    backend: &Path,
    id: &str,
    work_dir: &Path,
    arm: &IndependentB1Receipt<'_>,
) -> Result<Value> {
    identifier(id)?;
    validate_local_test_path(work_dir, backend, LocalTestPathKind::NewDirectory)?;
    let directory = new_path(backend, work_dir, "独立 B1 参考计划 work_dir")?;
    arm.require_disjoint(&directory)?;
    let source = SourceProjection::load(backend, &arm.source_request)?;
    let target = TargetProjection::load(&arm.target_request)?;
    let reference = project(
        id,
        RestoreInputSide::Candidate,
        &directory,
        &arm.target_request,
        &source,
        &target,
    )?;
    validate_independent_b1_plan(&reference, backend, &arm.target_request)?;
    for side in ["source", "target"] {
        let runtime = Path::new(super::document::text(
            &reference[side]["runtime_dir"],
            "参考运行目录",
        )?);
        disjoint_work_dir(&directory, [runtime.to_owned()])?;
    }
    source.assert_unchanged(backend, &arm.source_request)?;
    target.assert_unchanged(&arm.target_request)?;
    Ok(reference)
}

fn project(
    id: &str,
    side: RestoreInputSide,
    directory: &Path,
    request: &TargetRequest,
    source: &SourceProjection,
    configuration: &TargetProjection,
) -> Result<Value> {
    let declared = object(&request.document.value["target"], "新目标")?;
    let selected = object(&request.selected, "审阅单侧资源")?;
    let mut target = declared.clone();
    for field in ["runtime_dir", "api_url", "worker_ready_url", "frontend_url"] {
        target.insert(
            field.to_owned(),
            selected
                .get(field)
                .cloned()
                .ok_or_else(|| format!("审阅单侧资源缺少 {field}"))?,
        );
    }
    target.insert("s3".into(), configuration.target["s3"].clone());
    let plan = json!({
        "format_version":1, "id":id, "target_side":side.as_str(),
        "work_dir":directory.to_string_lossy(), "tools":source.tools, "source":source.source, "target":target,
    });
    if declared.get("scope_id") != plan["target"].get("scope_id")
        || declared.get("databases") != plan["target"].get("databases")
    {
        return Err("参考计划没有绑定当前 source-generation 或 fresh 物理目标".into());
    }
    Ok(plan)
}

pub(super) fn validate_publication_path(work_dir: &Path, output: &Path) -> Result<()> {
    let work_dir = canonical_path_from_parent(work_dir, "参考计划 work_dir")?;
    let output = canonical_path_from_parent(output, "参考计划输出")?;
    let same = if cfg!(windows) {
        let key = |path: &Path| {
            path.to_string_lossy()
                .trim_end_matches([' ', '.'])
                .to_lowercase()
        };
        key(&work_dir) == key(&output)
    } else {
        work_dir == output
    };
    if same {
        return Err("参考计划 --work-dir 与 --output 必须使用不同位置".into());
    }
    Ok(())
}

fn canonical_path_from_parent(value: &Path, label: &str) -> Result<PathBuf> {
    let parent = value.parent().ok_or_else(|| format!("{label}缺少父目录"))?;
    if !parent.is_dir() {
        return Err(format!("{label}的父目录必须已经存在").into());
    }
    let parent = plain_path(fs::canonicalize(parent)?);
    let name = value
        .file_name()
        .ok_or_else(|| format!("{label}缺少文件名"))?;
    Ok(parent.join(name))
}

fn new_path(backend: &Path, value: &Path, label: &str) -> Result<PathBuf> {
    if value.exists() {
        return Err(format!("{label}已存在，拒绝覆盖").into());
    }
    let result = canonical_path_from_parent(value, label)?;
    if !result.starts_with(plain_path(fs::canonicalize(backend.join(".local-tests"))?)) {
        return Err(format!("{label}必须位于当前后端 .local-tests").into());
    }
    Ok(result)
}

fn independent_work_dir(
    directory: &Path,
    source_request: &Map<String, Value>,
    selected: &Map<String, Value>,
    arm: &arm::Arm,
    fresh_path: &Path,
) -> Result<()> {
    let mut protected = Vec::new();
    if let Some(value) = source_request
        .get("source")
        .and_then(|value| value.get("runtime_dir"))
        .and_then(Value::as_str)
    {
        protected.push(PathBuf::from(value));
    }
    if let Some(value) = selected.get("runtime_dir").and_then(Value::as_str) {
        protected.push(PathBuf::from(value));
    }
    protected.push(
        fresh_path
            .parent()
            .ok_or("fresh target 核验缺少父目录")?
            .to_path_buf(),
    );
    if let Some(value) = arm
        .manifest
        .value
        .get("copy_directory")
        .and_then(Value::as_str)
    {
        protected.push(PathBuf::from(value));
    }
    disjoint_work_dir(directory, protected)
}

fn disjoint_work_dir(directory: &Path, protected: impl IntoIterator<Item = PathBuf>) -> Result<()> {
    let resolved = plain_path(fs::canonicalize(
        directory.parent().ok_or("工作目录缺少父目录")?,
    )?)
    .join(directory.file_name().ok_or("工作目录缺少名称")?);
    for path in protected {
        let path = plain_path(fs::canonicalize(&path).unwrap_or(path));
        if overlaps(&resolved, &path) {
            return Err("参考计划 work_dir 必须与来源、目标、复制和核验目录完全分离".into());
        }
    }
    Ok(())
}

fn overlaps(first: &Path, second: &Path) -> bool {
    first == second || first.starts_with(second) || second.starts_with(first)
}

fn plain_path(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    if let Some(value) = path.to_str().and_then(|value| {
        value
            .strip_prefix(r"\\?\UNC\")
            .map(|value| format!(r"\\{value}"))
            .or_else(|| value.strip_prefix(r"\\?\").map(ToOwned::to_owned))
    }) {
        return PathBuf::from(value);
    }
    path
}

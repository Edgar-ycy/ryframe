use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{
    document::{Document, array, boolean, exact, integer, object, text},
    domain::validate_local_optional,
    fresh, source,
    target::{self, TargetRequest, clone_name},
};
use crate::{
    Result,
    recovery::{
        clone::native::lifecycle::manifest,
        fixture_control::source_pair::execution::HeldTargetSource,
    },
};

pub(crate) use fresh::Fresh;

const RESULT_FIELDS: [&str; 20] = [
    "status",
    "request",
    "manifest",
    "source_registration",
    "source_request",
    "source_storage",
    "generation_verified",
    "initialized",
    "target_environment",
    "target_registration",
    "target_initialized_files",
    "target_storage_run",
    "target_side",
    "remote_writes",
    "outbox_drained",
    "restore_qualified",
    "review_successor",
    "source_export",
    "source_export_result",
    "source_generation",
];

const REQUEST_FIELDS: [&str; 14] = [
    "format_version",
    "kind",
    "source_registration",
    "id",
    "initialized",
    "target_registration",
    "target_initialized_files",
    "target_environment",
    "copy_directory",
    "build_bridges",
    "review_successor",
    "source_export",
    "source_export_result",
    "source_generation",
];

pub(crate) struct Arm {
    pub(crate) result: Document,
    pub(crate) request: Document,
    pub(crate) manifest: Document,
    pub(crate) source_request: Document,
    pub(crate) target: TargetRequest,
}

pub(crate) fn load(
    backend: &Path,
    arm_path: &Path,
    fresh_path: &Path,
    side: &str,
) -> Result<(Arm, Fresh)> {
    load_with_source(backend, arm_path, fresh_path, side, None)
}

pub(super) fn load_with_source(
    backend: &Path,
    arm_path: &Path,
    fresh_path: &Path,
    side: &str,
    held: Option<&HeldTargetSource>,
) -> Result<(Arm, Fresh)> {
    let result = super::document::read(arm_path, backend, "arm 输入")?;
    validate_result_status(&result, side)?;
    let result_object = exact(&result.value, &result_fields(&result.value), "arm 结果")?;
    let (_, request) = super::document::bound_file(
        backend,
        result_object.get("request").unwrap(),
        "arm 输入请求",
    )?;
    let mut request_fields = REQUEST_FIELDS.to_vec();
    if request.value.get("source_rebind").is_some() {
        request_fields.push("source_rebind");
    }
    let request_object = exact(&request.value, &request_fields, "arm 输入请求")?;
    if request_object.get("format_version") != Some(&Value::from(1))
        || request_object.get("kind").and_then(Value::as_str)
            != Some("devex-clone-seed-successor-arm-input")
        || request_object.get("id").and_then(Value::as_str).is_none()
    {
        return Err("正式恢复只接受绑定共享 source-export 的 successor arm-input".into());
    }
    clone_name(text(request_object.get("id").unwrap(), "arm 输入 ID")?)?;
    for field in [
        "source_registration",
        "initialized",
        "target_registration",
        "target_initialized_files",
        "target_environment",
        "review_successor",
        "source_export",
        "source_export_result",
        "source_generation",
    ] {
        super::document::bound_file(
            backend,
            request_object.get(field).unwrap(),
            &format!("arm 输入绑定 {field}"),
        )?;
    }
    for field in [
        "source_registration",
        "initialized",
        "target_environment",
        "target_registration",
        "target_initialized_files",
    ] {
        if result_object.get(field) != request_object.get(field) {
            return Err("successor arm-input 外层结果未完整绑定当前来源与 fresh 目标".into());
        }
    }
    for field in [
        "review_successor",
        "source_export",
        "source_export_result",
        "source_generation",
    ] {
        if result_object.get(field) != request_object.get(field) {
            return Err("successor arm-input 外层结果未完整绑定当前共享导出".into());
        }
    }
    if result_object.get("source_rebind") != request_object.get("source_rebind") {
        return Err("successor arm-input 外层结果未绑定同一存储重绑定".into());
    }
    let (_, manifest) = super::document::bound_file(
        backend,
        result_object.get("manifest").unwrap(),
        "arm 输入清单",
    )?;
    validate_manifest(backend, &manifest.value, &request.value)?;
    let (_, source_request) = super::document::bound_file(
        backend,
        result_object.get("source_request").unwrap(),
        "共享源导出请求",
    )?;
    source::validate(backend, &source_request.value)?;
    let effective_storage = result_object.get("target_storage_run").unwrap();
    let registration = read_target_lifecycle(
        backend,
        request_object.get("target_registration").unwrap(),
        request_object.get("initialized").unwrap(),
        request_object.get("target_initialized_files").unwrap(),
        request_object.get("target_environment").unwrap(),
        effective_storage,
    )?;
    let manifest_object = object(&manifest.value, "arm 输入清单")?;
    if manifest_object.get("target_storage_run") != Some(effective_storage) {
        return Err("successor arm-input 清单的 target_storage_run 不一致".into());
    }
    if manifest_object.get("copy_directory") != request_object.get("copy_directory") {
        return Err("successor arm-input 清单的 copy_directory 不一致".into());
    }
    if manifest_object.get("build_bridges") != request_object.get("build_bridges") {
        return Err("successor arm-input 清单的 build_bridges 不一致".into());
    }
    let (_, target_request_document) = super::document::bound_file(
        backend,
        registration
            .request
            .as_ref()
            .ok_or("fresh registration 缺少请求")?,
        "fresh target 请求",
    )?;
    let target = match held {
        Some(held) => {
            target::validate_with_source(backend, target_request_document, side, Some(held))?
        }
        None => target::validate(backend, target_request_document, side)?,
    };
    if target.selected.get("backend_dir").and_then(Value::as_str)
        != Some(target.execution.to_string_lossy().as_ref())
    {
        return Err("正式恢复输入未绑定同一 arm、目标侧或维护源码".into());
    }
    let initialized = registration.initialized;
    let fresh = fresh::validate(
        backend,
        fresh_path,
        &result_object["initialized"],
        &initialized,
        &target.document.value,
    )?;
    result.assert_unchanged()?;
    request.assert_unchanged()?;
    manifest.assert_unchanged()?;
    source_request.assert_unchanged()?;
    Ok((
        Arm {
            result,
            request,
            manifest,
            source_request,
            target,
        },
        fresh,
    ))
}

fn result_fields(value: &Value) -> Vec<&str> {
    let mut fields = RESULT_FIELDS.to_vec();
    if value.get("source_rebind").is_some() {
        fields.push("source_rebind");
    }
    fields
}

fn validate_result_status(result: &Document, side: &str) -> Result<()> {
    let result = object(&result.value, "arm 结果")?;
    if result.get("status").and_then(Value::as_str) != Some("seed_arm_input_published")
        || result.get("target_side").and_then(Value::as_str) != Some(side)
        || integer(
            result
                .get("remote_writes")
                .ok_or("arm 结果缺少 remote_writes")?,
            "arm 远端写入",
        )? != 0
        || !boolean(
            result
                .get("outbox_drained")
                .ok_or("arm 结果缺少 outbox_drained")?,
            "arm outbox",
        )?
        || boolean(
            result
                .get("restore_qualified")
                .ok_or("arm 结果缺少 restore_qualified")?,
            "arm restore_qualified",
        )?
    {
        return Err("successor arm-input 外层结果状态或目标侧无效".into());
    }
    Ok(())
}

struct Registration {
    request: Option<Value>,
    initialized: Value,
}

fn read_target_lifecycle(
    backend: &Path,
    registration_binding: &Value,
    initialized_binding: &Value,
    files_binding: &Value,
    environment_binding: &Value,
    storage_run: &Value,
) -> Result<Registration> {
    let (_, registration_document) =
        super::document::bound_file(backend, registration_binding, "fresh target registration")?;
    let registration = exact(
        &registration_document.value,
        &[
            "format_version",
            "kind",
            "request",
            "environment",
            "storage_run",
            "target_directory",
        ],
        "fresh target registration",
    )?;
    if registration.get("format_version") != Some(&Value::from(1))
        || registration.get("kind").and_then(Value::as_str)
            != Some("devex-clone-fresh-target-registration")
        || registration_document
            .path
            .file_name()
            .and_then(|name| name.to_str())
            != Some("registration.json")
    {
        return Err("arm 目标必须来自 fresh-target registration".into());
    }
    let workspace = registration_document
        .path
        .parent()
        .ok_or("registration 缺少目录")?;
    let target_dir = PathBuf::from(text(
        registration.get("target_directory").unwrap(),
        "fresh target 目录",
    )?);
    validate_local_optional(
        &target_dir,
        &backend.join(".local-tests"),
        "fresh target 目录",
    )?;
    if target_dir != workspace.join("target") {
        return Err("arm 目标目录不属于 fresh-target workspace".into());
    }
    let (_, initialized) =
        super::document::bound_file(backend, initialized_binding, "fresh target initialized")?;
    if initialized.path != target_dir.join("initialized.json") {
        return Err("arm 目标 initialized.json 不属于 fresh-target registration".into());
    }
    read_initialized_files(backend, files_binding, registration_binding, workspace)?;
    super::document::bound_file(backend, environment_binding, "fresh target 环境")?;
    if registration.get("environment") != Some(environment_binding) {
        return Err("arm 目标环境不属于 fresh-target registration".into());
    }
    manifest::validate_target_storage(backend, registration_binding, storage_run)?;
    Ok(Registration {
        request: registration.get("request").cloned(),
        initialized: initialized.value,
    })
}

fn read_initialized_files(
    backend: &Path,
    files_binding: &Value,
    registration_binding: &Value,
    workspace: &Path,
) -> Result<()> {
    let (_, files) =
        super::document::bound_file(backend, files_binding, "fresh target initialized-files")?;
    if files.path != workspace.join("initialized-files.json") {
        return Err("arm 目标文件快照不属于 fresh-target workspace".into());
    }
    let snapshot = exact(
        &files.value,
        &[
            "format_version",
            "kind",
            "registration",
            "predecessor",
            "files",
        ],
        "fresh target initialized-files",
    )?;
    if snapshot.get("format_version") != Some(&Value::from(1))
        || snapshot.get("kind").and_then(Value::as_str)
            != Some("devex-clone-fresh-target-initialized-files")
        || snapshot.get("registration") != Some(registration_binding)
    {
        return Err("arm 目标 initialized-files 未绑定当前完整初始化目录".into());
    }
    super::document::bound_file(
        backend,
        snapshot.get("predecessor").unwrap(),
        "fresh target prepared-files",
    )?;
    array(snapshot.get("files").unwrap(), "fresh target 文件列表")?;
    Ok(())
}

fn validate_manifest(backend: &Path, value: &Value, request: &Value) -> Result<()> {
    let mut fields = vec![
        "format_version",
        "kind",
        "id",
        "source_request",
        "source_export",
        "initialized",
        "source_environment",
        "target_environment",
        "copy_directory",
        "copy_stage",
        "build_bridges",
        "source_registration",
        "target_registration",
        "target_initialized_files",
        "target_storage_run",
        "review_successor",
        "source_export_result",
        "source_generation",
    ];
    if request.get("source_rebind").is_some() {
        fields.push("source_rebind");
    }
    let manifest = exact(value, &fields, "arm 输入清单")?;
    if manifest.get("format_version") != Some(&Value::from(1))
        || manifest.get("kind").and_then(Value::as_str) != Some("devex-clone-run")
        || manifest.get("copy_stage").and_then(Value::as_str) != Some("seed_to_arm")
    {
        return Err("successor arm-input 清单类型或阶段无效".into());
    }
    for field in [
        "id",
        "initialized",
        "target_environment",
        "source_registration",
        "target_registration",
        "target_initialized_files",
        "review_successor",
        "source_export",
        "source_export_result",
        "source_generation",
    ] {
        if manifest.get(field) != request.get(field) {
            return Err(format!("successor arm-input 清单的 {field} 与请求不一致").into());
        }
    }
    if manifest.get("source_rebind") != request.get("source_rebind") {
        return Err("successor arm-input 清单与当前存储重绑定不一致".into());
    }
    for field in [
        "source_request",
        "source_export",
        "initialized",
        "source_environment",
        "target_environment",
        "source_registration",
        "target_registration",
        "target_initialized_files",
        "review_successor",
        "source_export_result",
        "source_generation",
    ] {
        super::document::bound_file(backend, manifest.get(field).unwrap(), "arm 清单绑定")?;
    }
    if let Some(source_rebind) = manifest.get("source_rebind") {
        super::document::bound_file(backend, source_rebind, "arm 存储重绑定")?;
    }
    array(manifest.get("build_bridges").unwrap(), "arm 构建桥接")?;
    validate_local_optional(
        Path::new(text(manifest.get("copy_directory").unwrap(), "复制目录")?),
        &backend.join(".local-tests"),
        "复制目录",
    )
}

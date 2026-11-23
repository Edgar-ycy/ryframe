//! 完整参考目标计划的只读预览、原文件核验和独占发布。
use std::path::{Path, PathBuf};

#[cfg(test)]
use std::fs;

use serde_json::{Value, json};

use super::{inputs, path, text};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::{
        fixture_control::successor::arm_target,
        inputs::{
            chain::{self, Chain},
            document::{self, Document},
            reference,
            target_plan::{
                self,
                evidence::{Frame, NAMES, directory_identity},
                independent,
            },
        },
        runtime::dataset_authority::reads::Window,
        source::evidence::comparison,
    },
};

#[cfg(test)]
use target_plan::evidence::check_fresh_parent;

pub(super) fn run(backend: &Path, request: &Value, reference: Document) -> Result<Value> {
    let window = Window::direct()?;
    let result = execute(backend, request, &reference, &window);
    window.finish(result)
}

fn execute(
    backend: &Path,
    request: &Value,
    reference: &Document,
    window: &Window,
) -> Result<Value> {
    let output = request.get("output").map(path).transpose()?;
    if request["command"] != "plan"
        || request["write"] != output.is_some()
        || output.is_some() && request.get("target_plan").is_some()
        || reference.path.as_path() != path(&request["plan"])?
    {
        return Err("完整参考计划模式、写入授权或参考文件绑定不同".into());
    }
    if let Some(output) = output {
        validate_local_test_path(output, backend, LocalTestPathKind::OutputFile)?;
    }
    let frame = read_frame(backend, request, reference)?;
    let result = (|| {
        frame.check()?;
        if reference::is_independent_b1(&reference.value) {
            return execute_independent_b1(backend, &frame, output, window);
        }
        let (chain, mut value) = derive(backend, &frame)?;
        inputs::backup::verify_frozen_source(backend, &chain.backup)?;
        comparison::verify(backend, &chain.comparison_document.value, true)?;
        verify_arm(backend, &chain)?;
        frame.check_bindings(&value)?;
        frame.verify_fresh_inventory(backend, &chain.arm.target.document.value)?;
        // 旧 Python 输出及其摘要请求都递归排序。即便依赖打开 preserve_order 也保持原字节顺序。
        value.sort_all_objects();
        if let Some(output) = output {
            let backup = path(&chain.backup.result["backup_root"])?;
            return publish(backend, output, backup, &value, &frame, window);
        }
        if frame.target.is_some() {
            return result_summary(&value, None);
        }
        Ok(value)
    })();
    finish(result, frame.check())
}

fn execute_independent_b1(
    backend: &Path,
    frame: &Frame<'_>,
    output: Option<&Path>,
    window: &Window,
) -> Result<Value> {
    let (evidence, mut value) = independent::verify(backend, frame, output)?;
    let result = (|| {
        value.sort_all_objects();
        if let Some(output) = output {
            let backup = path(&evidence.backup.result["backup_root"])?;
            return publish(backend, output, backup, &value, frame, window);
        }
        if frame.target.is_some() {
            return result_summary(&value, None);
        }
        Ok(value)
    })();
    finish(result, evidence.check())
}

fn verify_arm(backend: &Path, chain: &Chain) -> Result<()> {
    let request = &chain.arm.request.value;
    let result = &chain.arm.result.value;
    let manifest = &chain.arm.manifest.value;
    let verified = comparison::published::verified(
        backend,
        &request["source_export_result"],
        Some(&request["review_successor"]),
    )?;
    let checked = (|| {
        let identity = &verified.identity;
        let source = &verified.source;
        if identity != &chain.comparison["source_export"]
            || identity["source_registration"] != request["source_registration"]
            || identity["source_rebind"] != request["source_rebind"]
            || identity["source_generation"] != request["source_generation"]
            || identity["export"] != request["source_export"]
            || result["source_request"] != source["source_request"]
            || result["source_storage"] != source["registration"]["source_storage"]
            || result["generation_verified"] != source["generation_verified"]
            || manifest["source_request"] != source["source_request"]
            || manifest["source_environment"] != source["source_environment"]
        {
            return Err("arm 结果、原清单或请求未绑定同一已发布共享来源".into());
        }

        let target = arm_target::for_input(backend, request, source, true, false)?;
        if result["target_side"].as_str() != Some(target.side.as_str())
            || result["target_storage_run"] != target.generation
        {
            return Err("arm 结果未绑定原登记目标侧及完整存储代次".into());
        }
        Ok(())
    })();
    finish(checked, verified.check(backend))
}

fn read_frame<'a>(backend: &Path, request: &Value, reference: &'a Document) -> Result<Frame<'a>> {
    if let Some(target) = request.get("target_plan") {
        let target = document::read(path(target)?, backend, "原正式目标计划")?;
        return Frame::published(backend, reference, target);
    }
    let [backup, comparison, arm, fresh, product] = NAMES.map(|field| path(&request[field]));
    Frame::explicit(
        backend,
        reference,
        [backup?, comparison?, arm?, fresh?, product?],
    )
}

fn derive(backend: &Path, frame: &Frame<'_>) -> Result<(Chain, Value)> {
    if let Some(target) = &frame.target {
        let (chain, value, _) =
            target_plan::verify(backend, frame.reference.clone(), &target.path)?;
        return Ok((chain, value));
    }
    let [backup, comparison, arm, fresh, product] = frame.evidence.as_slice() else {
        return Err("参考目标计划必须绑定五项原证据".into());
    };
    let chain = chain::load_with_reference(
        backend,
        frame.reference.clone(),
        &backup.path,
        &comparison.path,
        &arm.path,
        &fresh.path,
        text(&frame.reference.value["target_side"])?,
    )?;
    let value = target_plan::capture(backend, &chain, product)?;
    Ok((chain, value))
}

fn publish(
    backend: &Path,
    output: &Path,
    backup: &Path,
    value: &Value,
    frame: &Frame<'_>,
    window: &Window,
) -> Result<Value> {
    let initialized = document::binding_shape(&value["fresh_target"]["initialized"], "原初始化")?;
    let fresh = initialized.path.parent().ok_or("原初始化缺少证据目录")?;
    let mut protected = vec![backup, fresh];
    if reference::is_independent_b1(&frame.reference.value) {
        let [_, _, _, verify, _] = frame.evidence.as_slice() else {
            return Err("参考目标计划必须绑定五项原证据".into());
        };
        protected.push(verify.path.parent().ok_or("原目标核验缺少证据目录")?);
    }
    let (output, identity) = output_path(backend, output, &protected)?;
    let parent = output.parent().ok_or("目标计划输出缺少父目录")?;
    inputs::publish::publish_verified(&output, value, || {
        window.finish(frame.check())?;
        Ok(check_output_parent(parent, &protected)? == identity)
    })?;
    let published = document::read(&output, backend, "已发布正式目标计划")?;
    if published.value != *value || published.raw != document::canonical_bytes(value)? {
        return Err("正式目标计划发布后原始字节不同，保留现场".into());
    }
    result_summary(&published.value, Some(&output))
}

fn output_path(
    backend: &Path,
    selected: &Path,
    protected: &[&Path],
) -> Result<(PathBuf, (u64, u64))> {
    let output = validate_local_test_path(selected, backend, LocalTestPathKind::OutputFile)?;
    let parent = output.parent().ok_or("目标计划输出缺少父目录")?;
    let identity = check_output_parent(parent, protected)?;
    Ok((output, identity))
}

fn check_output_parent(parent: &Path, protected: &[&Path]) -> Result<(u64, u64)> {
    let identity = directory_identity(parent)?;
    let protected = protected
        .iter()
        .map(|path| directory_identity(path))
        .collect::<Result<Vec<_>>>()?;
    // 比较真实目录身份，Windows 大小写或长路径别名不能绕过原证据目录边界。
    for ancestor in parent.ancestors() {
        if protected.contains(&directory_identity(ancestor)?) {
            return Err("目标计划输出不得混入备份或 fresh-target 原证据".into());
        }
    }
    Ok(identity)
}

fn result_summary(value: &Value, output: Option<&Path>) -> Result<Value> {
    let mut result = json!({
        "status":if output.is_some() { "target_plan_published" } else { "target_plan_verified" },
        "target_side":value["target_side"], "target_plan_sha256":document::hash(value)?,
        "restore_success":false,
    });
    if let Some(output) = output {
        result["output"] = json!(output);
    }
    Ok(result)
}

fn finish<T>(result: Result<T>, after: Result<()>) -> Result<T> {
    match (result, after) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(after)) => {
            Err(format!("{error}；参考目标计划原始输入后验同时失败：{after}").into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_plan_summary_has_only_original_verify_and_publish_fields() -> Result<()> {
        let value = json!({"target_side":"base"});
        let digest = document::hash(&value)?;
        assert_eq!(
            result_summary(&value, None)?,
            json!({
                "status":"target_plan_verified","target_side":"base","target_plan_sha256":digest,"restore_success":false,
            })
        );
        let output = Path::new("target-plan.json");
        assert_eq!(
            result_summary(&value, Some(output))?,
            json!({
                "status":"target_plan_published","output":output,"target_side":"base",
                "target_plan_sha256":digest,"restore_success":false,
            })
        );
        Ok(())
    }

    #[test]
    fn target_plan_output_retains_sorted_utf8_with_original_indentation() -> Result<()> {
        let mut value = json!({"z":"汉字","a":{"z":false,"a":1}});
        value.sort_all_objects();
        assert_eq!(
            document::canonical_bytes(&value)?,
            "{\n  \"a\": {\n    \"a\": 1,\n    \"z\": false\n  },\n  \"z\": \"汉字\"\n}\n"
                .as_bytes()
        );
        Ok(())
    }

    #[test]
    fn fresh_verify_parent_rejects_retained_failure_and_initialization_lock() -> Result<()> {
        let local = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&local)?;
        let fixture = tempfile::tempdir_in(local)?;
        let verify = fixture.path().join("verify.json");
        let identity = check_fresh_parent(&verify)?;
        let failure = fixture.path().join("failure.json");
        fs::write(&failure, b"original")?;
        assert!(check_fresh_parent(&verify).is_err());
        fs::remove_file(&failure)?;
        let lock = fixture.path().join("initialize.lock");
        fs::create_dir(&lock)?;
        assert!(check_fresh_parent(&verify).is_err());
        fs::remove_dir(&lock)?;
        assert_eq!(check_fresh_parent(&verify)?, identity);
        Ok(())
    }

    #[test]
    fn target_plan_output_requires_a_new_file_outside_original_evidence() -> Result<()> {
        let local = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&local)?;
        let fixture = tempfile::tempdir_in(local)?;
        let backend = fixture.path();
        let work = backend.join(".local-tests");
        let backup = work.join("backup");
        let fresh = work.join("fresh/target");
        let observation = work.join("fresh/observation");
        let output = work.join("plans");
        for directory in [&backup, &fresh, &observation, &output] {
            fs::create_dir_all(directory)?;
        }
        let protected = [backup.as_path(), fresh.as_path(), observation.as_path()];
        let path = output.join("target.json");
        assert_eq!(output_path(backend, &path, &protected)?.0, path);
        assert!(!path.exists());
        for directory in protected {
            assert!(output_path(backend, &directory.join("target.json"), &protected).is_err());
        }
        assert!(output_path(backend, &work.join("missing/target.json"), &protected).is_err());
        fs::write(&path, b"original")?;
        assert!(output_path(backend, &path, &protected).is_err());
        assert_eq!(fs::read(&path)?, b"original");
        Ok(())
    }

    #[test]
    fn target_plan_preserves_original_bindings_and_publishes_once() -> Result<()> {
        let local = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&local)?;
        let fixture = tempfile::tempdir_in(local)?;
        let backend = fixture.path();
        let work = backend.join(".local-tests");
        let backup = work.join("backup");
        let fresh = work.join("fresh");
        for directory in [&backup, &fresh] {
            fs::create_dir_all(directory)?;
        }
        let write = |path: &Path, value: Value| -> Result<Document> {
            fs::write(path, document::canonical_bytes(&value)?)?;
            document::read(path, backend, "目标计划测试输入")
        };
        let reference = write(&work.join("reference.json"), json!({"target_side":"base"}))?;
        let receipt = write(&work.join("backup.json"), json!({"backup":true}))?;
        let comparison = write(&work.join("comparison.json"), json!({"comparison":true}))?;
        let arm = write(&work.join("arm.json"), json!({"arm":true}))?;
        let verify = write(&fresh.join("verify.json"), json!({"verify":true}))?;
        let initialized = write(&fresh.join("initialized.json"), json!({"initialized":true}))?;
        let product = write(&work.join("product.json"), json!({"product":true}))?;
        let request = json!({
            "backup_receipt":receipt.path,
            "comparison_sources":comparison.path,
            "arm_input":arm.path,
            "fresh_target_verify":verify.path,
            "product_plan":product.path,
        });
        let value = json!({
            "target_side":"base",
            "comparison_arm":"b0",
            "backup_receipt":document::binding(&receipt),
            "comparison_sources":document::binding(&comparison),
            "arm_input":document::binding(&arm),
            "fresh_target":{
                "verify":document::binding(&verify),
                "initialized":document::binding(&initialized),
            },
            "product_plan_file":document::binding(&product),
        });
        let window = Window::direct()?;
        let frame = read_frame(backend, &request, &reference)?;
        document::bound_file(backend, &value["fresh_target"]["initialized"], "原初始化")?;
        frame.check_bindings(&value)?;
        let output = work.join("target-plan-output.json");
        assert!(!output.exists());
        let summary = publish(backend, &output, &backup, &value, &frame, &window)?;
        assert_eq!(summary["status"], "target_plan_published");
        assert_eq!(summary["target_plan_sha256"], document::hash(&value)?);
        let published = document::canonical_bytes(&value)?;
        assert_eq!(fs::read(&output)?, published);
        assert!(publish(backend, &output, &backup, &value, &frame, &window).is_err());
        assert_eq!(fs::read(&output)?, published);

        let verify = read_frame(backend, &json!({"target_plan":output}), &reference)?;
        let selected = verify.target.as_ref().unwrap();
        let stored = &selected.value;
        verify.check_bindings(stored)?;
        independent::require_readback(Some(selected), &value)?;
        for pointer in [
            "/backup_receipt",
            "/comparison_sources",
            "/arm_input",
            "/fresh_target/verify",
            "/product_plan_file",
        ] {
            let mut replaced = value.clone();
            *replaced.pointer_mut(pointer).unwrap() = document::binding(&reference);
            assert!(frame.check_bindings(&replaced).is_err(), "{pointer}");
            assert!(
                independent::require_readback(Some(selected), &replaced).is_err(),
                "{pointer}"
            );
        }
        window.finish(verify.check())?;
        assert_eq!(*stored, value);
        Ok(())
    }
}

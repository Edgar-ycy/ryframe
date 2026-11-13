//! 完整参考目标计划的只读预览、原文件核验和独占发布。
use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::{inputs, native, path, text};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path, validate_local_test_path},
    recovery::{
        fixture_control::successor::arm_target,
        inputs::{
            chain::{self, Chain},
            document::{self, Document},
            target_plan,
        },
        monitoring::execution::ProtectedInputs,
        runtime::dataset_authority::reads::Window,
        source::evidence::comparison,
    },
};

const EVIDENCE: [&str; 5] = [
    "backup_receipt",
    "comparison_sources",
    "arm_input",
    "fresh_target_verify",
    "product_plan",
];

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
    let frame = Frame::read(backend, request, reference)?;
    let result = (|| {
        frame.check()?;
        let (chain, mut value) = frame.derive(backend)?;
        inputs::backup::verify_frozen_source(backend, &chain.backup)?;
        comparison::verify(backend, &chain.comparison_document.value, true)?;
        verify_arm(backend, &chain)?;
        frame.check_bindings(&value)?;
        verify_fresh_inventory(backend, &chain, fresh_evidence(&frame.evidence)?)?;
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

fn verify_fresh_inventory(backend: &Path, chain: &Chain, verify: &Document) -> Result<()> {
    let initial = &verify.value["inventory"];
    let parent = verify.path.parent().ok_or("目标 verify 缺少父目录")?;
    native::target_verify::evidence_protocol::inventory_history::verify(
        backend,
        parent,
        &json!({
            "initial":initial,
            "request":chain.arm.target.document.value,
            "resumed":false,
            "inventory_directory":parent.join("inventory"),
        }),
    )?;
    Ok(())
}

fn fresh_evidence(evidence: &[Document]) -> Result<&Document> {
    let [_, _, _, verify, _] = evidence else {
        return Err("参考目标计划必须绑定五项原证据".into());
    };
    Ok(verify)
}

fn check_fresh_parent(verify: &Path) -> Result<(u64, u64)> {
    let parent = verify.parent().ok_or("目标 verify 缺少父目录")?;
    let identity = directory_identity(parent)?;
    for name in ["failure.json", "initialize.lock"] {
        match fs::symlink_metadata(parent.join(name)) {
            Ok(_) => return Err("目标 verify 目录存在失败证据或初始化锁，不能消费残留结果".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    if directory_identity(parent)? != identity {
        return Err("目标 verify 原父目录身份在核验期间发生变化".into());
    }
    Ok(identity)
}

struct Frame<'a> {
    reference: &'a Document,
    target: Option<Document>,
    evidence: Vec<Document>,
    fresh_parent: (u64, u64),
    held: ProtectedInputs,
}

impl<'a> Frame<'a> {
    fn read(backend: &Path, request: &Value, reference: &'a Document) -> Result<Self> {
        validate_local_test_path(&reference.path, backend, LocalTestPathKind::ExistingFile)?;
        let target = request
            .get("target_plan")
            .map(|selected| document::read(path(selected)?, backend, "原正式目标计划"))
            .transpose()?;
        let evidence = match &target {
            Some(target) => descriptors(&target.value)
                .into_iter()
                .zip(EVIDENCE)
                .map(|(value, label)| {
                    document::bound_file(backend, value, label).map(|(_, document)| document)
                })
                .collect::<Result<Vec<_>>>()?,
            None => EVIDENCE
                .into_iter()
                .map(|field| document::read(path(&request[field])?, backend, field))
                .collect::<Result<Vec<_>>>()?,
        };
        let held = ProtectedInputs::open(
            std::iter::once(reference)
                .chain(target.iter())
                .chain(&evidence)
                .map(|file| file.path.clone()),
        )?;
        let fresh_parent = check_fresh_parent(&fresh_evidence(&evidence)?.path)?;
        Ok(Self {
            reference,
            target,
            evidence,
            fresh_parent,
            held,
        })
    }

    fn derive(&self, backend: &Path) -> Result<(Chain, Value)> {
        if let Some(target) = &self.target {
            let (chain, value, _) =
                target_plan::verify(backend, self.reference.clone(), &target.path)?;
            return Ok((chain, value));
        }
        let [backup, comparison, arm, fresh, product] = self.evidence.as_slice() else {
            return Err("参考目标计划必须绑定五项原证据".into());
        };
        let chain = chain::load_with_reference(
            backend,
            self.reference.clone(),
            &backup.path,
            &comparison.path,
            &arm.path,
            &fresh.path,
            text(&self.reference.value["target_side"])?,
        )?;
        let value = target_plan::capture(backend, &chain, product)?;
        Ok((chain, value))
    }

    fn check(&self) -> Result<()> {
        self.held.assert_unchanged()?;
        if check_fresh_parent(&fresh_evidence(&self.evidence)?.path)? != self.fresh_parent {
            return Err("目标 verify 原父目录身份发生变化".into());
        }
        for input in std::iter::once(self.reference)
            .chain(self.target.iter())
            .chain(&self.evidence)
        {
            input.assert_unchanged()?;
        }
        Ok(())
    }

    fn check_bindings(&self, value: &Value) -> Result<()> {
        if self.evidence.len() != EVIDENCE.len()
            || descriptors(value)
                .into_iter()
                .zip(&self.evidence)
                .any(|(binding, document)| *binding != document::binding(document))
        {
            return Err("参考目标计划没有保持显式五项证据的原路径和原始字节".into());
        }
        Ok(())
    }
}

fn descriptors(value: &Value) -> [&Value; 5] {
    [
        &value["backup_receipt"],
        &value["comparison_sources"],
        &value["arm_input"],
        &value["fresh_target"]["verify"],
        &value["product_plan_file"],
    ]
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
    let (output, identity) = output_path(backend, output, backup, fresh)?;
    let parent = output.parent().ok_or("目标计划输出缺少父目录")?;
    inputs::publish::publish_verified(&output, value, || {
        window.finish(frame.check())?;
        Ok(check_output_parent(parent, backup, fresh)? == identity)
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
    backup: &Path,
    fresh: &Path,
) -> Result<(PathBuf, (u64, u64))> {
    let output = validate_local_test_path(selected, backend, LocalTestPathKind::OutputFile)?;
    let parent = output.parent().ok_or("目标计划输出缺少父目录")?;
    let identity = check_output_parent(parent, backup, fresh)?;
    Ok((output, identity))
}

fn check_output_parent(parent: &Path, backup: &Path, fresh: &Path) -> Result<(u64, u64)> {
    let identity = directory_identity(parent)?;
    let protected = [directory_identity(backup)?, directory_identity(fresh)?];
    // 比较真实目录身份，Windows 大小写或长路径别名不能绕过原证据目录边界。
    for ancestor in parent.ancestors() {
        if protected.contains(&directory_identity(ancestor)?) {
            return Err("目标计划输出不得混入备份或 fresh-target 初始化证据".into());
        }
    }
    Ok(identity)
}

fn directory_identity(path: &Path) -> Result<(u64, u64)> {
    validate_absolute_path(path, LocalTestPathKind::ExistingDirectory)?;
    native::resources::common::device_inode_for(path, &fs::symlink_metadata(path)?)
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
        let output = work.join("plans");
        for directory in [&backup, &fresh, &output] {
            fs::create_dir_all(directory)?;
        }
        let path = output.join("target.json");
        assert_eq!(output_path(backend, &path, &backup, &fresh)?.0, path);
        assert!(!path.exists());
        for protected in [&backup, &fresh] {
            assert!(output_path(backend, &protected.join("target.json"), &backup, &fresh).is_err());
        }
        assert!(output_path(backend, &work.join("missing/target.json"), &backup, &fresh).is_err());
        fs::write(&path, b"original")?;
        assert!(output_path(backend, &path, &backup, &fresh).is_err());
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
        let frame = Frame::read(backend, &request, &reference)?;
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

        let verify = Frame::read(backend, &json!({"target_plan":output}), &reference)?;
        let stored = &verify.target.as_ref().unwrap().value;
        verify.check_bindings(stored)?;
        window.finish(verify.check())?;
        assert_eq!(*stored, value);
        Ok(())
    }
}

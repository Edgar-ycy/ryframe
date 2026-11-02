use std::path::Path;

use serde_json::{Value, json};

use super::super::{file_binding, output_value, path_value, read_json, write_new};
use crate::{
    Result, local_test_path::LocalTestPathKind,
    recovery::clone::native::cache::review_tools::observe,
};

const LIMIT: u64 = 4 * 1024 * 1024;

pub(in crate::recovery::fixture_control) fn run(
    root: &Path,
    review: &Path,
    output: &Path,
) -> Result<Value> {
    renew_with_observer(root, review, output, observe)?;
    Ok(json!({"status":"planned","services_started":false,"remote_writes":0}))
}

fn renew_with_observer(
    root: &Path,
    review: &Path,
    output: &Path,
    mut observe: impl FnMut(&Path, &Value) -> Result<Value>,
) -> Result<Value> {
    let mut stage = "输入核验";
    let result: Result<Value> = (|| {
        path_value(review, root, LocalTestPathKind::ExistingFile, "审阅输入")?;
        let predecessor = file_binding(review, "审阅 predecessor")?;
        let original = read_json(review, "审阅 predecessor", LIMIT)?;
        let mut structural = original.as_object().ok_or("审阅根节点不是对象")?.clone();
        if structural
            .get("ready_for_execution")
            .and_then(Value::as_bool)
            .is_none()
        {
            return Err("ready_for_execution 必须是布尔值".into());
        }
        structural.insert("ready_for_execution".to_owned(), json!(true));
        super::super::review::validate_review(&Value::Object(structural))?;
        if original["ready_for_execution"] == true {
            super::guards::preflight_binding(&original)?;
        }
        stage = "首次工具观察";
        let observed = observe(root, &original)?;
        stage = "输出路径核验";
        output_value(output, root, "新审阅收据")?;
        stage = "首次输入重核";
        unchanged(review, &original, &predecessor)?;
        stage = "收据构造与合同核验";
        let revised = receipt(&original, &observed, &predecessor)?;
        stage = "终末工具观察";
        let terminal = observe(root, &original)?;
        stage = "终末输入与工具重核";
        if terminal != observed {
            return Err("审阅当前工具在 ready 签发前发生变化".into());
        }
        unchanged(review, &original, &predecessor)?;
        stage = "本地收据发布";
        write_new(output, &revised)?;
        Ok(revised)
    })();
    result.map_err(|source| Box::new(ReviewFailure { stage, source }) as Box<dyn std::error::Error>)
}

fn receipt(original: &Value, observed: &Value, predecessor: &Value) -> Result<Value> {
    let mut revised = original.clone();
    // original 已在首次观察前通过完整结构校验；服务节点保持原结构。
    let tools = revised["tools"].as_object_mut().ok_or("审阅工具不是对象")?;
    for (name, value) in observed.as_object().ok_or("观察工具不是对象")? {
        tools.insert(name.clone(), value.clone());
    }
    tools.remove("redis_python");
    // Ready 的资源字段已通过原绑定守卫；保持 JSON 原文，不重写等价路径。
    if original["ready_for_execution"] == false {
        let scope = format!(
            "services-{}",
            original["scopes"]["seed"]["scope_id"]
                .as_str()
                .ok_or("seed scope 缺失")?
        );
        let process = super::guards::service_run(original)?
            .join("rustfs")
            .join("process.json");
        revised["services"]["rustfs"]["scope_id"] = json!(scope);
        revised["services"]["rustfs"]["process_receipt"] = json!(process);
    }
    revised["ready_for_execution"] = json!(true);
    revised["preflight"] = json!({
            "format_version":1,"kind":"reference-fixture-tool-preflight","status":"verified",
            "supersedes":predecessor,"tools":observed,
    });
    super::super::review::validate_review(&revised)?;
    super::guards::preflight_binding(&revised)?;
    super::guards::successor(original, &revised)?;
    Ok(revised)
}

fn unchanged(path: &Path, original: &Value, binding: &Value) -> Result<()> {
    if read_json(path, "审阅 predecessor", LIMIT)? != *original
        || file_binding(path, "审阅 predecessor")? != *binding
    {
        return Err("审阅 predecessor 在工具预检期间发生变化".into());
    }
    Ok(())
}

#[derive(Debug)]
struct ReviewFailure {
    stage: &'static str,
    source: Box<dyn std::error::Error>,
}

impl std::fmt::Display for ReviewFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use crate::recovery::clone::native::cache;
        let error = self.source.as_ref();
        let reason = if let Some(message) = cache::primitive_message(error) {
            message
        } else if let Some(error) = error.downcast_ref::<std::io::Error>() {
            format!(
                "I/O kind={:?}，系统码={:?}",
                error.kind(),
                error.raw_os_error()
            )
        } else if let Some(error) = error.downcast_ref::<serde_json::Error>() {
            format!(
                "JSON {:?}，行={}，列={}",
                error.classify(),
                error.line(),
                error.column()
            )
        } else {
            // 上游观察器及工具配置已输出固定安全原因，不拼接配置值或子进程输出。
            error.to_string()
        };
        write!(
            f,
            "环境审阅阶段“{}”失败：{reason}；核对 --review 的受控普通文件、原始字节摘要和前后像；核对 config/tools.toml 与 .local-tests/tools.toml 的 verification.linux_observer、WSL入口、工具文件摘要及完整源码/ELF绑定",
            self.stage
        )?;
        if self.stage == "本地收据发布" {
            f.write_str("；create_new/写入/同步未确认完成，输出可能已存在或包含部分文件；保留输出与原审阅，核对文件内容、权限及磁盘状态，禁止覆盖或自动重放，不得盲目重试")
        } else {
            f.write_str(
                "；保留原审阅与失败现场，确认来源和配置差异后再显式决定后续操作，禁止自动重放",
            )
        }
    }
}

impl std::error::Error for ReviewFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/recovery/fixture_control/environment/review.rs"]
mod tests;

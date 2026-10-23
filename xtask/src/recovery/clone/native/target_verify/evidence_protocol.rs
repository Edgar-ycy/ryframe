//! 初始化控制器复用原生只读证据核验；请求不经过参数或环境变量。

use std::{
    io::{Read, Write},
    path::Path,
};

use serde_json::{Value, json};

use super::super::json as json_io;
use crate::Result;

const MARKER: &str = "__target-evidence";
const MAX_BYTES: u64 = 1024 * 1024;

pub(crate) fn run_hidden_if_requested() -> Option<i32> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments.first().map(String::as_str) != Some(MARKER) {
        return None;
    }
    let mut bytes = Vec::new();
    let result = (|| {
        if arguments.len() != 1 {
            return Err("目标证据协议只接受标准输入请求".into());
        }
        std::io::stdin()
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)?;
        execute(&bytes)
    })();
    let (code, response) = match result {
        Ok(value) => (0, json!({"ok":true,"result":value})),
        Err(error) => (
            1,
            json!({"ok":false,"error":{
            "category":"TargetEvidenceRejected",
            "message":safe_message(error.as_ref()),
            "remote_writes":0,"automatic_retry":false}}),
        ),
    };
    let written: Result<()> =
        serde_json::to_vec(&response)
            .map_err(Into::into)
            .and_then(|mut bytes| {
                bytes.push(b'\n');
                std::io::stdout()
                    .lock()
                    .write_all(&bytes)
                    .map_err(Into::into)
            });
    Some(if written.is_ok() { code } else { 1 })
}

fn safe_message(error: &dyn std::error::Error) -> &str {
    // 只公开固定诊断，解析错误、文件路径与任意输入值均不进入协议输出。
    const DIAGNOSTICS: &[&str] = &[
        "reset 报告或账本不属于当前精确 plan",
        "reset 有未完成阶段或锁释放证据缺失",
        "reset 存在未确认资源",
        "fresh 初始化必须本次 completed；reused、失败或释放中断均拒绝",
        "reset 上一版账本不属于最终账本的直接前像",
        "reset 上一版账本含有非 release 阶段变化",
        "reset 上一版账本没有证明最终 release 的直接前像",
        "reset 完成报告数据库集合不符",
        "reset 完成报告 Redis 或对象集合不符",
        "reset plan 命令收据与固定清单不一致",
        "初始库存文件不属于本代次",
        "初始库存原始文件或摘要变化",
        "迁移前缀命令、退出状态或成功输出无效",
        "迁移前缀包含未知命令收据",
        "迁移前缀包含重复命令收据",
        "迁移收据越过首个缺失操作，不能证明连续前缀",
        "迁移续作没有完成固定序列",
        "迁移前缀的首个缺失操作必须是只读 verify",
        "续作操作没有产生唯一成功命令收据",
    ];
    let reason = error.to_string();
    DIAGNOSTICS
        .iter()
        .copied()
        .find(|message| *message == reason)
        .unwrap_or(
            "目标证据核验失败；核对绑定路径、计划、账本及直接前像，保留现场，不自动重放或清理",
        )
}

pub(crate) fn execute(bytes: &[u8]) -> Result<Value> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_BYTES {
        return Err("目标证据请求为空或超过 1 MiB".into());
    }
    let value = json_io::parse_fragment(std::str::from_utf8(bytes)?, "目标证据请求")?;
    let operation = json_io::text(&value["operation"], "目标证据操作")?;
    let extra = match operation {
        "reset-completed" => &["manifest", "sha"][..],
        "reset-plan" => &["path", "executable", "plan"][..],
        "history" => &[][..],
        "initial-inventory" => &["receipt", "resumed"][..],
        "migration-operations" => &["executable"][..],
        "migration-prefix" => &["operations", "complete"][..],
        "migration-operation-record" => &["migration_operation"][..],
        _ => return Err("未知目标证据操作".into()),
    };
    let mut fields = vec!["format_version", "operation", "backend", "target"];
    fields.extend(extra);
    json_io::exact(&value, &fields, "目标证据请求")?;
    if value["format_version"].as_u64() != Some(1) {
        return Err("目标证据请求版本无效".into());
    }
    let backend = Path::new(json_io::text(&value["backend"], "目标证据后端")?);
    let target = Path::new(json_io::text(&value["target"], "目标证据目录")?);
    crate::local_test_path::validate_local_test_path(
        target,
        backend,
        crate::local_test_path::LocalTestPathKind::ExistingDirectory,
    )?;
    match operation {
        "migration-operations" => {
            super::resume_prefix::operations(json_io::text(&value["executable"], "迁移程序")?)
        }
        "migration-prefix" => super::resume_prefix::prefix(
            backend,
            target,
            &value["operations"],
            value["complete"]
                .as_bool()
                .ok_or("迁移完成标志必须是布尔值")?,
        ),
        "migration-operation-record" => {
            super::resume_prefix::operation_record(backend, target, &value["migration_operation"])
        }
        "reset-completed" => super::reset::completed(
            backend,
            target,
            &value["manifest"],
            json_io::text(&value["sha"], "reset 摘要")?,
        ),
        "reset-plan" => super::reset::plan_receipt(
            backend,
            target,
            Path::new(json_io::text(&value["path"], "reset 命令收据")?),
            json_io::text(&value["executable"], "reset 程序")?,
            &value["plan"],
        ),
        "history" => super::proof::history(backend, target),
        "initial-inventory" => {
            super::proof::initial_inventory(
                backend,
                target,
                &value["receipt"],
                value["resumed"]
                    .as_bool()
                    .ok_or("库存续作标志必须为布尔值")?,
            )?;
            Ok(json!({"status":"initial_inventory_verified","remote_writes":0}))
        }
        _ => unreachable!(),
    }
}

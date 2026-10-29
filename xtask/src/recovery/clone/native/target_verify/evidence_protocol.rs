//! 初始化控制器复用原生只读证据核验；请求不经过参数或环境变量。

use std::{
    io::{Read, Write},
    path::Path,
};

use serde_json::{Value, json};

use super::super::json as json_io;
use crate::Result;

#[path = "inventory_history.rs"]
pub(super) mod inventory_history;

const MARKER: &str = "__target-evidence";
const MAX_BYTES: u64 = 1024 * 1024;

pub(crate) fn run_hidden_if_requested() -> Option<i32> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let binding = arguments.first().map(String::as_str) == Some("__target-binding");
    if !binding && arguments.first().map(String::as_str) != Some(MARKER) {
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
        if binding {
            super::binding::execute(&bytes)
        } else {
            execute(&bytes)
        }
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

const DIAGNOSTICS: &[&str] = &[
    super::storage::kernel::OBSERVATION_ERROR,
    super::database::model::TABLE_SET_ERROR,
    super::evidence_reset::MANIFEST_VERSION_ERROR,
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
    "fresh 创建阶段 intent 或 confirmed 不属于固定目标",
    "fresh 创建阶段包含缺失、重复命名或未知证据",
    "fresh 初始化前存在未收尾的 prepare 续作",
    "fresh 初始化包含未登记的 intent 或 confirmed 证据",
    "fresh 目标 failure 不是可续作的只读 prepare 失败",
    "fresh 目标 prepare 与旧失败之间缺少显式续作绑定",
    "fresh 目标 prepare 结果不属于固定 registration",
    "fresh 目标 prepare 续作 intent 无效",
    "fresh 目标 prepare 续作前像变化",
    "fresh 目标 prepare 续作时间不属于初始化前因果链",
    "fresh 目标 prepare 续作确认缺少最终 prepare.json",
    "fresh 目标 prepare 续作结果不属于原 intent",
    "fresh 目标 prepare 续作结果无效",
    "fresh 目标 prepare 续作结果晚于初始化开始",
    "fresh 目标 prepare 续作结果缺少原 intent",
    "fresh 目标 prepare 输出不是普通目录",
    "fresh 目标包含无法归属的 prepare 续作记录",
    "fresh 目标同一 prepare 续作同时存在多个结果",
    "fresh 目标已发布 prepare 与当前只读前像不同",
    "fresh 目标没有唯一可归属的初始化失败",
    "fresh 目标请求副本在 prepare 续作期间变化",
    "prepare 续作前像包含初始化写入证据",
    "prepare 续作确认不属于当前固定结果",
    "prepare 请求文件已变化",
    "reset execute 收据与固定计划或完成报告不一致",
    "reset 命令收据没有证明成功完成",
    "reset 必须为 plan、复核和 execute 各保留唯一命令收据",
    "reset 阶段包含未知命令收据",
    "reset 阶段包含缺失、重复命名或未知证据",
    "实际 reset plan 来源或引导范围不符",
    "reset plan 包含额外或缺失数据库",
    "reset plan 数据库合并或重复",
    "reset plan 数据库实际目标、baseline 或 owner 不符",
    "reset plan 对象端点或五桶不符",
    "reset plan 对象前缀越界",
    "reset plan Redis namespace、端点或 sentinel 不符",
    super::reset::TERMINAL_VERSION_ERROR,
    "维护 CLI 在执行前发生变化",
    "初始化失败、仍持锁或观察目录无效",
    "初始化失败之前的 prepare 失败尚未显式收尾",
    "初始化期间 prepare 证据变化",
    "原 reset 两次固定计划证据不一致",
    "迁移初始化只允许一个未确认只读操作的首次续作前驱",
    "迁移后继只读观察命令收据无效",
    "迁移后继的 MySQL prepare 基线无效",
    "迁移后继的 Redis prepare 基线无效",
    "迁移后继的 Redis 重启观察与固定代次不符",
    "迁移后继的前驱 attempt 命名无效",
    "迁移后继的前驱失败、只读操作或证据闭集无效",
    "迁移后继的前驱失败路径无效",
    "迁移后继的前驱活动操作缺失",
    "迁移后继的首个缺失只读操作 intent 不存在",
    "迁移后继缺少首次续作的两轮 Redis 代次观察",
    "迁移后继缺少首次续作的两轮存储代次观察",
    "迁移后继缺少首次续作的数据库前像观察",
    "迁移续作判定期间目标证据树发生变化",
    "迁移续作的 Redis owner 前像无效",
    "迁移续作的初始化前像摘要无效",
    "迁移续作的对象 owner 前像无效",
    "迁移续作的数据库或对象前像不属于固定 fresh 目标",
    "迁移续作请求或 prepare 不属于固定 registration",
    "Redis PID 无效",
    "Redis started 证据不同于已发布重启结果",
    "Redis 内核身份输出无效",
    "Redis 标记重绑证据内容或因果顺序无效",
    "Redis 进程名称缺失",
    "Redis 重启请求没有绑定原 fresh 代次",
    "RustFS started 证据不同于已发布重启结果",
    "RustFS 重启请求没有绑定原 fresh 代次",
    "fresh prepare MySQL 缺失观察不属于固定目标",
    "fresh prepare MySQL 观察数量无效",
    "fresh prepare Redis /proc stat 观察不属于固定进程",
    "fresh prepare Redis 内核观察不是五项两轮",
    "fresh prepare 原生对象只读观察收据无效",
    "fresh prepare 声明、来源或空资源前像无效",
    "fresh prepare 失败续作谱系没有完整确认",
    "fresh prepare 存储目录两轮观察不一致",
    "fresh prepare 存储目录观察数量无效",
    "fresh prepare 对象前缀并非明确空范围",
    "fresh prepare 对象桶启用了版本",
    "fresh prepare 对象观察未完整覆盖五桶",
    "fresh prepare 快照包含未声明的文件",
    "fresh prepare 快照包含目录、越界路径或变化文件",
    "fresh prepare 快照缺少固定请求或结果",
    "fresh 初始化 MySQL 前后像输出多重集无效",
    "fresh 初始化 MySQL 基线无效",
    "fresh 初始化 MySQL 观察命令无效",
    "fresh 初始化 MySQL 观察收据数量无效",
    "fresh 初始化 Redis 工具、配置或进程身份发生变化",
    "fresh 初始化包含未登记的普通文件或目录",
    "fresh 初始化原生对象只读观察收据无效",
    "fresh 初始化存储代次观察收据无效",
    "fresh 初始化对象 owner 命令、字节或结果无效",
    "fresh 初始化对象 owner 命令无效",
    "fresh 初始化对象 owner 证据未完整覆盖五桶",
    "fresh 初始化对象基线未覆盖五桶",
    "fresh 初始化对象观察命令不是逐桶精确多重集",
    "fresh 初始化控制互斥文件无效",
    "fresh 初始化缺少固定阶段文件",
    "fresh 目标 prepare 基线文件缺失或摘要变化",
    "fresh 目标 prepare 文件快照、registration 或 predecessor 绑定无效",
    "fresh 目标 prepare 文件快照或 registration 在读取期间变化",
    "fresh 目标存储二进制不属于审阅版本",
    "fresh 目标维护构建缺少完整二进制",
    "prepare 文件字段缺失或包含未知字段",
    "prepare 文件必须为对象",
    "prepare 文件快照",
    "prepare 文件快照字段缺失或包含未知字段",
    "prepare 文件快照必须为对象",
    "prepare 文件绑定",
    "prepare 文件绑定字段缺失或包含未知字段",
    "prepare 文件绑定必须为对象",
    "初始化开始字段缺失或包含未知字段",
    "初始化结果字段缺失或包含未知字段",
    "原生对象收据字段缺失或包含未知字段",
    "只读命令收据字段缺失或包含未知字段",
    "失败不是可证明前缀的迁移文件访问中断",
    "夹具存储与缓存恢复不是同一服务代次",
    "夹具存储代次字段缺失或包含未知字段",
    "夹具服务代次不是唯一成功且当前活跃 restart",
    "夹具服务代次结果不属于统一账本",
    "夹具服务重启 lineage 未绑定 fresh-target 的原始或已登记服务代次",
    "夹具缓存代次字段缺失或包含未知字段",
    "夹具重启绑定字段缺失或包含未知字段",
    "夹具重启缺少唯一 Redis 标记重绑确认",
    "存储恢复证据没有产生新的实际进程代次",
    "存储重启字段缺失或包含未知字段",
    "审阅绑定字段缺失或包含未知字段",
    "对象 owner 集合字段缺失或包含未知字段",
    "对象 owner 集合必须为对象",
    "操作系统错误字段缺失或包含未知字段",
    "标记 confirmed",
    "标记 confirmed字段缺失或包含未知字段",
    "标记 confirmed必须为对象",
    "标记 intent字段缺失或包含未知字段",
    "标记 intent必须为对象",
    "目标审阅 scope 不同",
    "目标登记字段缺失或包含未知字段",
    "续作操作 confirmed",
    "续作操作 confirmed字段缺失或包含未知字段",
    "续作操作 confirmed必须为对象",
    "续作操作 intent字段缺失或包含未知字段",
    "续作操作 intent必须为对象",
    "维护构建二进制字段缺失或包含未知字段",
    "维护构建二进制必须为对象",
    "缓存重启字段缺失或包含未知字段",
    "迁移 prepare字段缺失或包含未知字段",
    "迁移 prepare必须为对象",
    "迁移前驱 intent字段缺失或包含未知字段",
    "迁移前驱 intent必须为对象",
    "迁移前驱 started",
    "迁移前驱 started字段缺失或包含未知字段",
    "迁移前驱 started必须为对象",
    "迁移前驱失败字段缺失或包含未知字段",
    "迁移前驱操作字段缺失或包含未知字段",
    "迁移前驱活动操作字段缺失或包含未知字段",
    "迁移前驱活动操作必须为对象",
    "迁移原失败不满足只读续作条件",
    "迁移原失败字段缺失或包含未知字段",
    "迁移后继与首次失败续作代次不同",
    "迁移后继的存储 prepare 基线无效",
    "迁移成功 intent字段缺失或包含未知字段",
    "迁移成功 intent必须为对象",
    "迁移成功 reset 与已确认结果不同",
    "迁移成功 started",
    "迁移成功 started字段缺失或包含未知字段",
    "迁移成功 started必须为对象",
    "迁移成功命令收据字段缺失或包含未知字段",
    "迁移成功命令收据必须为对象",
    "迁移成功确认 attempt 无效",
    "迁移成功确认不是唯一结果",
    "迁移成功确认内容或因果顺序无效",
    "迁移成功确认包含缺失或未知续作文件",
    "迁移成功确认字段缺失或包含未知字段",
    "迁移成功确认没有从首个只读 verify 开始",
    "迁移成功确认的 intent 或 started 无效",
    "迁移成功确认的初始化像无效",
    "迁移操作不同于固定完整序列",
    "迁移操作必须符合固定序列结构",
    "迁移收据名称必须为 UTF-8",
    "迁移程序路径必须是非空且无控制字符的文本",
    "迁移续作 started 未绑定允许的当前存储代次",
    "迁移续作 started 的存储代次不同于可信发布证据",
    "迁移续作前像字段缺失或包含未知字段",
    "迁移续作对象字段缺失或包含未知字段",
    "迁移续作操作收据、确认或时间链无效",
    "迁移续作的存储恢复代次无效",
    "非夹具重启代次出现 Redis 标记重绑证据",
];

fn safe_message(error: &dyn std::error::Error) -> &str {
    // 只公开固定诊断，解析错误、文件路径与任意输入值均不进入协议输出。
    let reason = error.to_string();
    if let Some(message) = inventory_history::diagnostic(&reason) {
        return message;
    }
    if let Some(message) = super::resume::diagnostic(&reason) {
        return message;
    }
    if let Some(message) = super::binding::diagnostic(&reason) {
        return message;
    }
    if let Some(message) = super::cli::diagnostic(&reason) {
        return message;
    }
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
    if let Some(result) = super::resume::execute(bytes) {
        return result;
    }
    let value = json_io::parse_fragment(std::str::from_utf8(bytes)?, "目标证据请求")?;
    let operation = json_io::text(&value["operation"], "目标证据操作")?;
    if value.get("arguments").is_some() {
        json_io::exact(
            &value,
            &[
                "format_version",
                "operation",
                "backend",
                "target",
                "arguments",
            ],
            "目标 CLI 请求",
        )?;
        if value["format_version"].as_u64() != Some(1) {
            return Err("目标证据请求版本无效".into());
        }
        let backend = Path::new(json_io::text(&value["backend"], "目标证据后端")?);
        let target = Path::new(json_io::text(&value["target"], "目标证据目录")?);
        let kind = if operation == "cli-status" && target.is_file() {
            crate::local_test_path::LocalTestPathKind::ExistingFile
        } else {
            crate::local_test_path::LocalTestPathKind::StateDirectory
        };
        crate::local_test_path::validate_local_test_path(target, backend, kind)?;
        return super::cli::execute(backend, target, operation, &value["arguments"]);
    }
    let extra = match operation {
        "process-argv" => &["receipt", "identity"][..],
        "storage-generation" => &["expected", "actual", "runtime", "cache_runtime"][..],
        "storage-layout" => &["request", "storage", "runtime", "cache_runtime"][..],
        "history" | "reset-completed" => &[][..],
        "initial-inventory" => &["receipt", "resumed"][..],
        "inventory-history" => &["initial", "request", "resumed", "inventory_directory"][..],
        "migration-operations" => &["executable"][..],
        "migration-prefix" => &["operations", "complete"][..],
        "migration-operation-record" => &["migration_operation"][..],
        "migration-resume-state" => &["request_descriptor", "prepared_files", "owned_lock"][..],
        "migration-resume-confirmed" => &["initialized"][..],
        "creation-evidence" | "initial-stage-names" => &["request"][..],
        "reset-evidence" => &["request", "original"][..],
        "initialization-paths" => &["creation", "reset", "prefix"][..],
        "validate-stage-names" => &["expected"][..],
        "prepare-resume-stages" | "prepare-resume-records" | "initialization-failure" => {
            &["request_descriptor"][..]
        }
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
    dispatch(backend, target, operation, &value)
}

fn dispatch(backend: &Path, target: &Path, operation: &str, value: &Value) -> Result<Value> {
    match operation {
        "process-argv" | "storage-generation" | "storage-layout" => {
            super::storage::protocol::execute(backend, target, operation, value)
        }
        "inventory-history" => inventory_history::verify(backend, target, value),
        "migration-resume-state" => super::resume_state::state(
            backend,
            target,
            &value["request_descriptor"],
            &value["prepared_files"],
            value["owned_lock"]
                .as_bool()
                .ok_or("初始化锁标志必须为布尔值")?,
        ),
        "migration-resume-confirmed" => Ok(
            json!({"confirmed":super::resume_confirmed::confirmed(backend,target,&value["initialized"])}),
        ),
        "creation-evidence" => {
            super::evidence_creation::creation(backend, target, &value["request"])
        }
        "reset-evidence" => {
            if target.join("initialized.json").try_exists()? {
                super::evidence_reset::history(
                    backend,
                    target,
                    &value["request"],
                    &value["original"],
                )
            } else {
                super::evidence_reset::aggregate(
                    backend,
                    target,
                    &value["request"],
                    &value["original"],
                )
            }
        }
        "initialization-paths" => Ok(
            json!({"paths":super::evidence_creation::paths(target,&value["creation"],&value["reset"],&value["prefix"])?}),
        ),
        "initial-stage-names" => {
            Ok(json!({"names":super::evidence_creation::initial_stages(&value["request"])?}))
        }
        "validate-stage-names" => {
            let expected = json_io::array(&value["expected"], "初始化阶段名")?
                .iter()
                .map(|value| Ok(json_io::text(value, "初始化阶段名")?.to_owned()))
                .collect::<Result<std::collections::BTreeSet<_>>>()?;
            super::evidence_creation::stage_names(target, &expected)?;
            Ok(json!({"status":"stage_names_verified","remote_writes":0}))
        }
        "prepare-resume-stages" => Ok(
            json!({"names":super::evidence_prepare::stages(backend,target,&value["request_descriptor"])?}),
        ),
        "prepare-resume-records" => {
            super::evidence_prepare::records(backend, target, &value["request_descriptor"])
        }
        "initialization-failure" => {
            super::evidence_prepare::failure_path(backend, target, &value["request_descriptor"])
        }
        "migration-operations" => super::resume_prefix::operations(
            json_io::text(&value["executable"], "迁移程序")?,
            &json_io::read(&target.join("request.json"))?,
        ),
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
        "reset-completed" => reset_completed(backend, target),
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

fn reset_completed(backend: &Path, target: &Path) -> Result<Value> {
    let paths = ["request.json", "prepare.json", "initialized.json"].map(|name| target.join(name));
    let protected = super::guard::Inputs::open(paths.iter().cloned())?;
    let request = json_io::read(&paths[0])?;
    let prepared = json_io::read(&paths[1])?;
    let initialized = json_io::read(&paths[2])?;
    let result =
        super::evidence_reset::history(backend, target, &request, &prepared["generation"])?;
    let completed = &result["completed"];
    if initialized["status"] != "fresh_target_initialized"
        || initialized["generation"] != prepared["generation"]
        || initialized["id"] != request["id"]
        || initialized["scope_id"] != request["target"]["scope_id"]
        || *completed != initialized["reset"]
    {
        return Err("reset 完成投影与初始化终态不一致".into());
    }
    protected.check()?;
    Ok(completed.clone())
}

//! 只读任务证据的原生私有入口；所有业务关联与 SQL 由固定模型限定。

use std::{
    collections::BTreeMap,
    env,
    io::{BufReader, Read, Write},
};

use serde_json::{Value, json};

#[path = "job_evidence/artifact.rs"]
pub(crate) mod artifact;
#[path = "job_evidence/model.rs"]
pub(crate) mod model;
#[path = "job_evidence/mysql.rs"]
pub(crate) mod mysql;
#[path = "job_evidence/protocol.rs"]
pub(crate) mod protocol;
#[path = "job_evidence/sql.rs"]
pub(crate) mod sql;
#[path = "job_evidence/wait.rs"]
pub(crate) mod wait;

use protocol::{EvidenceResult, Session};

const MARKER: &str = "__job-evidence";

pub(crate) fn run_hidden_if_requested() -> Option<i32> {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    if arguments.first().map(String::as_str) != Some(MARKER) {
        return None;
    }
    let code = match arguments.get(1).map(String::as_str) {
        Some("collect") if arguments.len() == 2 => run_collect(),
        Some("wait") if arguments.len() == 2 => run_wait(),
        _ => {
            let _ = emit(failure("invalid_job_evidence_operation"));
            2
        }
    };
    Some(code)
}

fn run_collect() -> i32 {
    let mut input = Vec::new();
    let read = std::io::stdin()
        .take((protocol::INPUT_LIMIT + 1) as u64)
        .read_to_end(&mut input);
    let result = if read.is_err() || input.len() > protocol::INPUT_LIMIT {
        Err("collector_input_limit")
    } else {
        collect(&input, &environment())
    };
    let value = result.unwrap_or_else(failure);
    let success = value["valid"].as_bool() == Some(true);
    if emit(value).is_err() {
        return 1;
    }
    if success { 0 } else { 1 }
}

pub(crate) fn collect(
    input: &[u8],
    environment: &BTreeMap<String, String>,
) -> EvidenceResult<Value> {
    let request = protocol::parse(input, "invalid_collector_request")?;
    protocol::exact(
        &request,
        &["binding", "scope_id", "selection"],
        "invalid_collector_request",
    )?;
    let scope = request["scope_id"].as_str().ok_or("invalid_scope")?;
    protocol::binding(&request["binding"], scope, environment)?;
    model::selection(&request["selection"])?;
    let mut session = mysql::MysqlSession::open(&request["binding"])?;
    let result = protocol::collect(
        &mut session,
        &request["binding"],
        scope,
        &request["selection"],
    );
    let closed = Session::close(&mut session);
    match result {
        Ok(value) => closed.map(|()| value),
        Err(reason) => Err(reason),
    }
}

fn run_wait() -> i32 {
    let mut reader = BufReader::new(std::io::stdin());
    let result = (|| {
        let request = protocol::read_line(&mut reader)?.ok_or("invalid_wait_binding")?;
        protocol::exact(&request, &["binding", "scope_id"], "invalid_wait_binding")?;
        let scope = request["scope_id"].as_str().ok_or("invalid_scope")?;
        protocol::binding(&request["binding"], scope, &environment())?;
        let mut session = mysql::MysqlSession::open(&request["binding"])?;
        let result = wait::serve(&mut session, &request["binding"], scope, reader, emit);
        let closed = Session::close(&mut session);
        result.and(closed)
    })();
    match result {
        Ok(()) => 0,
        Err(reason) => {
            let _ = emit(failure(reason));
            1
        }
    }
}

pub(crate) fn failure(reason: &'static str) -> Value {
    json!({"format_version": 1, "valid": false, "reason": reason})
}

fn emit(mut value: Value) -> EvidenceResult<()> {
    if value["valid"].as_bool() == Some(false) {
        let suggestion = hint(value["reason"].as_str().unwrap_or_default());
        value
            .as_object_mut()
            .ok_or("database_completion_failed")?
            .insert("hint".into(), json!(suggestion));
    }
    value
        .as_object_mut()
        .ok_or("database_completion_failed")?
        .insert("format_version".into(), json!(1));
    let encoded = serde_json::to_vec(&value).map_err(|_| "database_completion_failed")?;
    let mut output = std::io::stdout().lock();
    output
        .write_all(&encoded)
        .and_then(|()| output.write_all(b"\n"))
        .and_then(|()| output.flush())
        .map_err(|_| "job_observer_output_failed")
}

fn environment() -> BTreeMap<String, String> {
    env::vars_os()
        // 此层仅核对变量存在性；密码保留 OsString 交由客户端构造入口读取。
        .filter_map(|(name, _)| Some((name.into_string().ok()?, String::new())))
        .collect()
}

fn hint(reason: &str) -> &'static str {
    match reason {
        "invalid_collector_request"
        | "invalid_wait_binding"
        | "invalid_wait_request"
        | "collector_input_limit"
        | "wait_input_limit" => "核对同一版本控制器产生的请求，检查输入是否完整以及大小限制。",
        "invalid_database_binding"
        | "invalid_control_binding"
        | "invalid_scope"
        | "invalid_server_identity" => {
            "检查 bindings.job_timings 的 control、server_uuid、mysql_client 和本次 scope_id。"
        }
        "missing_database_password_environment" | "mysql-password-missing" => {
            "在控制器环境中设置 control.password_env 指定的密码变量，变量名和凭据值须分别配置。"
        }
        "ambiguous_database_password_environment" | "mysql-environment-ambiguous" => {
            "清理同名但大小写不同的环境声明，保持 control.password_env 对应的唯一密码变量。"
        }
        "wrong_database_ownership" | "wrong_server_identity" | "mysql-ci-owner" => {
            "改用本次明确登记的隔离控制库，并核对服务器 UUID、scope 与 ownership 收据。"
        }
        "mysql-client-path" | "mysql-client-binary" => {
            "检查 bindings.job_timings.mysql_client 的绝对路径、普通二进制文件与执行权限。"
        }
        "mysql-timeout" | "mysql-close-timeout" => {
            "检查客户端进程、连接和管道的超时记录；先核对当前任务实际结果，再决定后续操作。"
        }
        "job_cancelled" => "业务任务已取消；核对任务状态，本周期不能计为成功样本。",
        "job_failed" => "按记录中的业务 ID 和全部尝试历史检查对应任务日志。",
        "attempt_result_unknown" | "missing_attempt_history" => {
            "尝试记录尚不能证明完整执行结果；核对领取序号及全部持久化尝试，不推断任务成功。"
        }
        _ => "按固定原因分类核对任务、尝试历史及采集日志，保留当前失败记录。",
    }
}

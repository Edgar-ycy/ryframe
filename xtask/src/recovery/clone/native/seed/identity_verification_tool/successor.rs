//! 只为已完成会话和解的身份核验追加独立工具登记。
use super::*;
use crate::recovery::clone::native::{post_process, target_verify::guard::Inputs};

#[path = "recovered_producer.rs"]
mod recovered_producer;
#[path = "successor_selection.rs"]
mod selection;
pub(crate) use selection::resolve;
pub(super) use selection::select;
use selection::{PreviousRecovery, previous_recovery, recovered_file, recovery_evidence};

const RECOVERY_REQUEST: &str = "devex-seed-identity-verification-recovery-request";
const RECOVERY_RETRY_REQUEST: &str = "devex-seed-identity-verification-recovery-retry-request";

pub(crate) fn path(run: &Path) -> PathBuf {
    run.join("seed-runtime/identity-verification-tool-successor.json")
}

pub(crate) fn recovery_path(run: &Path) -> PathBuf {
    run.join("seed-runtime/identity-verification-tool-successor-recovery.json")
}

pub(crate) fn recovery_retry_path(run: &Path, number: u64) -> PathBuf {
    output(run, number).join("identity-verification-tool-recovery-retry.json")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Canonical,
    Recovery,
    RecoveryRetry,
}

impl Kind {
    fn path(self, run: &Path, number: u64) -> PathBuf {
        match self {
            Self::Canonical => path(run),
            Self::Recovery => recovery_path(run),
            Self::RecoveryRetry => recovery_retry_path(run, number),
        }
    }
}

pub(super) struct Current<'a> {
    pub(super) row: &'a Value,
    pub(super) owner: &'a Value,
    pub(super) original: &'a Value,
    pub(super) request: &'a Value,
    pub(super) departments: &'a Value,
}

fn require_attempt(value: &Value, number: u64) -> Result<()> {
    if value["first_attempt"] != number {
        return Err("原会话和解只授权其绑定的首次后继身份核验阶段".into());
    }
    Ok(())
}

struct Authority {
    legacy: Legacy,
    request: Value,
    predecessor: Value,
    api_publication: Value,
    failed_controller: Value,
    prelaunch_failures: Vec<Value>,
    settlement_result: Value,
    session: Settlement,
    build: Value,
    recovered: Option<Recovered>,
    previous_recovery: Option<PreviousRecovery>,
}

struct Recovered {
    evidence: recovered_producer::RecoveredProducer,
    tool_sha256: Value,
}

impl Authority {
    fn load(
        backend: &Path,
        run: &Path,
        rows: &[Value],
        number: u64,
        active: &api_successor::Selected,
        request: &Value,
        kind: Kind,
    ) -> Result<Self> {
        if active.passed_attempt >= number {
            return Err("第二代 API 未在本次身份核验之前完成发布".into());
        }
        let failed = before(rows, active.restart_attempt)
            .find(|row| is_verify(row))
            .ok_or("缺少第二代 API 之前的原失败身份核验")?;
        let failure_number = number_of(&failed["number"])?;
        let predecessor = common::binding(&super::path(run))?;
        let legacy = Legacy::load(backend, run, rows, failure_number, &predecessor)?;
        if active.binding == legacy.history.api_publication
            || active.publication["registration"] != legacy.history.request["post_copy"]
        {
            return Err("只有同一登记下的第二代 API 可以登记独立核验工具后继".into());
        }
        let supplied = request_value(backend, request, &predecessor, kind)?;
        let settlement = settlement(
            backend,
            run,
            rows,
            number,
            active,
            &supplied["settlement_result"],
        )?;
        let failed_controller =
            failure_controller(backend, run, failed, &settlement.failed_verification)?;
        let recovered = (kind != Kind::Canonical)
            .then(|| {
                recovery_evidence(
                    backend,
                    run,
                    rows,
                    number,
                    &supplied,
                    &settlement,
                    &active.binding,
                )
            })
            .transpose()?;
        let retries = before(rows, number).filter(|row| {
            is_verify(row)
                && row["number"].as_u64().is_some_and(|n| {
                    n > failure_number
                        && recovered
                            .as_ref()
                            .is_none_or(|old| n != old.evidence.attempt)
                })
        });
        let prelaunch_failures = prelaunch_failures(backend, run, retries, settlement.number)?;
        let mut authority = Self {
            legacy,
            request: request.clone(),
            predecessor,
            api_publication: active.binding.clone(),
            failed_controller,
            prelaunch_failures,
            settlement_result: supplied["settlement_result"].clone(),
            session: settlement,
            build: supplied["build"].clone(),
            recovered,
            previous_recovery: None,
        };
        if kind == Kind::RecoveryRetry {
            authority.previous_recovery = Some(previous_recovery(
                backend, run, rows, number, &supplied, &authority,
            )?);
        }
        Ok(authority)
    }

    fn record(
        &self,
        backend: &Path,
        number: u64,
        controller: &Value,
        owner: &Value,
    ) -> Result<(Value, Value)> {
        let history = &self.legacy.history;
        let tool = continuation_tool::verify(backend, &self.build, PURPOSE, owner)?;
        if [
            &self.legacy.tool,
            &history.previous_tool,
            &history.request["identity_tool"],
        ]
        .into_iter()
        .any(|other| tool["sha256"] == other["sha256"])
            || self
                .recovered
                .as_ref()
                .is_some_and(|old| tool["sha256"] == old.tool_sha256)
            || self
                .previous_recovery
                .as_ref()
                .is_some_and(|old| tool["sha256"] == old.tool["sha256"])
        {
            return Err("独立核验工具后继必须使用已冻结的新版本".into());
        }
        let mut record = self.record_value(number, controller, &tool);
        if let Some(previous) = &self.previous_recovery {
            record["previous_recovery"] = previous.descriptor.clone();
        }
        Ok((record, tool))
    }

    fn record_value(&self, number: u64, controller: &Value, tool: &Value) -> Value {
        let history = &self.legacy.history;
        let mut record = json!({"format_version":1,"kind":"devex-seed-identity-verification-tool-successor",
            "run_manifest":history.manifest,"first_attempt":number,"controller":controller,
            "request":self.request,"predecessor":self.predecessor,
            "original_registration":history.original,"identity_plan":history.request["identity_plan"],
            "identity_state":history.request["identity_state"],"reconciliation":history.reconciliation,
            "preparation":history.preparation,"prepared_projection_sha256":history.prepared["projection_sha256"],
            "departments":history.departments,"previous_api_publication":history.api_publication,
            "api_publication":self.api_publication,"failed_verification":self.session.failed_verification,
            "prelaunch_failures":self.prelaunch_failures,
            "settlement_result":self.settlement_result,"session_receipt":self.session.receipt,
            "verifying_ledger_sha256":self.session.ledger_sha256,
            "build":self.build,"tool":tool});
        if let Some(recovered) = &self.recovered {
            record["kind"] = json!("devex-seed-identity-verification-tool-successor-recovery");
            record["recovered_producer"] = recovered.evidence.proof.clone();
        }
        record
    }

    fn check_verifying(&self, backend: &Path) -> Result<()> {
        let ledger = self.legacy.history.check_projection(backend, false)?;
        if ledger["state"] != "verifying"
            || ledger["ledger"]["sha256"] != self.session.ledger_sha256
        {
            return Err("新 API 身份核验必须接续与会话和解前像逐字节一致的 verifying 账本".into());
        }
        Ok(())
    }

    fn protect(&self, backend: &Path) -> Result<Inputs> {
        let history = &self.legacy.history;
        let descriptors = [
            &self.request,
            &self.predecessor,
            &self.api_publication,
            &self.session.failed_verification,
            &self.failed_controller,
            &self.settlement_result,
            &self.session.receipt,
            &self.session.request,
            &self.session.before,
            &self.build,
            &history.original,
            &history.preparation,
            &history.departments["stage"],
            &history.departments["plan"],
            &history.departments["prerequisite"],
            &history.api_publication,
        ];
        let paths = || -> Result<Vec<PathBuf>> {
            let mut paths = descriptors
                .iter()
                .copied()
                .chain(&self.prelaunch_failures)
                .chain(
                    self.previous_recovery
                        .iter()
                        .flat_map(|previous| previous.protected()),
                )
                .map(|binding| common::bound_file(backend, binding, "独立核验工具后继的不可变证据"))
                .collect::<Result<Vec<_>>>()?;
            if let Some(recovered) = &self.recovered {
                for descriptor in &recovered.evidence.descriptors {
                    paths.push(recovered_file(descriptor)?);
                }
            }
            if let Some(previous) = &self.previous_recovery {
                paths.push(
                    crate::recovery::clone::native::target_verify::input::request::external_file(
                        &previous.tool,
                    )?,
                );
            }
            Ok(paths)
        };
        let protected = Inputs::open(paths()?)?;
        paths()?;
        Ok(protected)
    }

    fn request_with_tool(self, tool: Value) -> Value {
        let mut selected = self.legacy.history.request;
        selected["identity_tool"] = tool;
        selected
    }
}

fn request_value(
    backend: &Path,
    request: &Value,
    predecessor: &Value,
    kind: Kind,
) -> Result<Value> {
    let value = common::read_bound(backend, request, "独立核验工具后继请求")?;
    let mut fields = vec![
        "format_version",
        "kind",
        "predecessor",
        "settlement_result",
        "build",
    ];
    let expected = match kind {
        Kind::Canonical => "devex-seed-identity-verification-successor-request",
        Kind::Recovery => {
            fields.extend(["recovered_result", "legacy_source"]);
            RECOVERY_REQUEST
        }
        Kind::RecoveryRetry => {
            fields.extend(["recovered_result", "legacy_source", "previous_recovery"]);
            RECOVERY_RETRY_REQUEST
        }
    };
    document::exact(&value, &fields, "独立核验工具后继请求")?;
    if value["format_version"] != 1
        || value["kind"] != expected
        || value["predecessor"] != *predecessor
    {
        return Err("独立核验工具后继请求未绑定原不可变工具登记".into());
    }
    Ok(value)
}

struct Settlement {
    number: u64,
    failed_verification: Value,
    receipt: Value,
    request: Value,
    before: Value,
    ledger_sha256: Value,
}

fn settlement(
    backend: &Path,
    run: &Path,
    rows: &[Value],
    number: u64,
    active: &api_successor::Selected,
    descriptor: &Value,
) -> Result<Settlement> {
    let row = before(rows, number)
        .find(|row| row["stage"] == "seed-runtime" && row["mode"] == identity_successor::MODE)
        .ok_or("独立核验工具后继缺少已通过的会话和解阶段")?;
    let settlement_number = number_of(&row["number"])?;
    if row["status"] != "passed"
        || settlement_number <= active.passed_attempt
        || row["result"] != *descriptor
    {
        return Err("会话和解并非第二代 API 发布后、身份核验前最后一次通过的正式和解".into());
    }
    let result = read_at(
        backend,
        descriptor,
        &state::result_path(run, settlement_number),
    )?;
    document::exact(
        &result,
        &[
            "status",
            "failed_verification",
            "api_publication",
            "session_receipt",
            "restore_qualified",
        ],
        "身份核验会话和解结果",
    )?;
    if result["status"] != "identity_verification_session_reconciled"
        || result["api_publication"] != active.binding
        || result["restore_qualified"] != false
    {
        return Err("会话和解结果不同于本次 API 或同一和解阶段的不可变收据".into());
    }
    let receipt = read_at(
        backend,
        &result["session_receipt"],
        &output(run, settlement_number).join("identity-session-reconciliation.json"),
    )?;
    let ledger_sha256 = verify_settlement_receipt(backend, &result, &receipt)?;
    Ok(Settlement {
        number: settlement_number,
        failed_verification: result["failed_verification"].clone(),
        receipt: result["session_receipt"].clone(),
        request: receipt["request"].clone(),
        before: receipt["before"].clone(),
        ledger_sha256,
    })
}

fn verify_settlement_receipt(backend: &Path, result: &Value, receipt: &Value) -> Result<Value> {
    document::exact(
        receipt,
        &[
            "format_version",
            "status",
            "request",
            "failed_verification",
            "api_publication",
            "before",
            "after",
            "final_observation",
            "http",
            "audit",
            "restore_qualified",
        ],
        "身份核验会话和解收据",
    )?;
    let request = common::read_bound(backend, &receipt["request"], "原会话和解请求")?;
    let request_id = document::text(&receipt["http"]["request_id"], "强退 HTTP 请求 ID")?;
    let confirmed = uuid::Uuid::parse_str(request_id)
        .is_ok_and(|id| id.get_version_num() == 7 && id.to_string() == request_id);
    if receipt["format_version"] != 1
        || receipt["status"] != result["status"]
        || receipt["failed_verification"] != result["failed_verification"]
        || receipt["api_publication"] != result["api_publication"]
        || receipt["restore_qualified"] != false
        || receipt["http"]["request_started"] != true
        || receipt["http"]["admin_session_closed"] != true
        || !confirmed
        || receipt["audit"]["kind"] != "devex-identity-session-force-logout-audit"
        || receipt["audit"]["request_id"] != request_id
        || request["failed_verification"] != receipt["failed_verification"]
        || request["api_publication"] != receipt["api_publication"]
        || request["before"] != receipt["before"]
    {
        return Err("会话核验收据不属于同一原失败、新 API 和已闭合的原会话动作".into());
    }
    let before = common::read_bound(backend, &receipt["before"], "原遗留会话前像")?;
    let ledger = document::text(&before["IdentityLedgerSha"], "原 verifying 账本摘要")?;
    let ledger_sha256 = json!(ledger.to_ascii_lowercase());
    let [prior] = document::array(&before["MatchingRows"], "原单会话前像")? else {
        return Err("原会话前像没有唯一匹配会话".into());
    };
    let sid = json!(document::text(&prior["SidSha"], "原会话摘要")?.to_ascii_lowercase());
    if !digest_shape(&ledger_sha256)
        || !digest_shape(&sid)
        || ["after", "final_observation"]
            .into_iter()
            .any(|field| receipt[field]["cleared"] != true || receipt[field]["sid_sha256"] != sid)
    {
        return Err("原 verifying 账本摘要或同一会话的两次完整后像无效".into());
    }
    Ok(ledger_sha256)
}

fn prelaunch_failures<'a>(
    backend: &Path,
    run: &Path,
    rows: impl IntoIterator<Item = &'a Value>,
    settlement: u64,
) -> Result<Vec<Value>> {
    let mut proofs = Vec::new();
    for row in rows {
        let number = number_of(&row["number"])?;
        if number <= settlement {
            return Err("会话和解前的其他身份核验不可作为未启动重试".into());
        }
        let receipt = common::binding(&run.join(format!("failure-{number:04}.json")))?;
        let controller = failure_controller(backend, run, row, &receipt)?;
        post_process::files::absent(&state::result_path(run, number))?;
        post_process::files::absent(&output(run, number).join("session-launch.json"))?;
        if !post_process::inspect_historical(backend, run, row)?.is_null() {
            return Err("曾启动的身份核验必须保留原生产者并按原操作核对".into());
        }
        proofs.extend([receipt, controller]);
    }
    Ok(proofs)
}

fn failure_controller(
    backend: &Path,
    run: &Path,
    row: &Value,
    descriptor: &Value,
) -> Result<Value> {
    let number = number_of(&row["number"])?;
    if row["status"] != "failed"
        || !row["result"].is_null()
        || row["error_type"] != "StageError"
        || row["finished_at"].as_str().is_none_or(str::is_empty)
    {
        return Err("第二代核验工具只能接续原未产生正式结果的身份核验失败".into());
    }
    let filename = format!("failure-{number:04}.json");
    let receipt = read_at(backend, descriptor, &run.join(filename))?;
    let (controller, _) = verify_controller(run, row, false)?;
    if receipt
        != json!({"format_version":2,"kind":"devex-stage-failure","attempt":number,
        "stage":"seed-runtime","mode":MODE,"error_type":"StageError",
        "operation_id":"clone.seed-runtime.identities-verify","controller":controller})
    {
        return Err("会话和解没有绑定原身份核验失败文件与固定控制器".into());
    }
    Ok(controller)
}

#[cfg(test)]
#[test]
fn settlement_proof_keeps_the_same_failed_attempt_session_and_verifying_ledger() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let backend = fixture.path();
    let directory = backend.join(".local-tests/run");
    fs::create_dir_all(&directory)?;
    let before = directory.join("before.json");
    document::write_new(
        &before,
        &json!({"IdentityLedgerSha":"A".repeat(64),
        "MatchingRows":[{"SidSha":"B".repeat(64)}]}),
    )?;
    let before = common::binding(&before)?;
    let failed = json!({"attempt":65});
    let api = json!({"attempt":66});
    let request = directory.join("request.json");
    document::write_new(
        &request,
        &json!({"failed_verification":failed,"api_publication":api,"before":before}),
    )?;
    let request = common::binding(&request)?;
    let status = "identity_verification_session_reconciled";
    let result = json!({"status":status,"failed_verification":failed,"api_publication":api});
    let observed = json!({"cleared":true,"sid_sha256":"b".repeat(64)});
    let request_id = "019ab52d-7311-7abc-8def-1234567890ab";
    let mut receipt = json!({"format_version":1,"status":status,"request":request,
        "failed_verification":failed,"api_publication":api,"before":before,"after":observed,
        "final_observation":observed,"http":{"request_started":true,"admin_session_closed":true,
        "request_id":request_id},"audit":{"kind":"devex-identity-session-force-logout-audit",
        "request_id":request_id},"restore_qualified":false});
    assert_eq!(
        verify_settlement_receipt(backend, &result, &receipt)?,
        "a".repeat(64)
    );
    receipt["final_observation"]["sid_sha256"] = json!("c".repeat(64));
    assert!(verify_settlement_receipt(backend, &result, &receipt).is_err());
    receipt["final_observation"] = observed;
    receipt["audit"]["request_id"] = json!("019ab52d-7311-7abc-8def-1234567890ac");
    assert!(verify_settlement_receipt(backend, &result, &receipt).is_err());
    receipt["audit"]["request_id"] = json!(request_id);
    receipt["failed_verification"] = json!({"attempt":64});
    assert!(verify_settlement_receipt(backend, &result, &receipt).is_err());
    Ok(())
}

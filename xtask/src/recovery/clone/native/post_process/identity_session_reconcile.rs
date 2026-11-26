//! 在原验证停窗后，仅收口已核对的单个遗留会话。
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::{
    api_successor,
    department_api::identity_session_reconciliation_api_proof,
    identity_session_redis::{self, Subject},
    seed_capacity::Guard,
};
use crate::{
    Result,
    data::performance_identities::{
        AdminForceLogout, AdminForceLogoutOutcome, force_logout_as_system_admin,
    },
    recovery::clone::native::{
        json as document,
        lifecycle::state,
        resources::common,
        target_verify::{fresh::selection, guard::Inputs, storage::StorageInputs},
    },
};

mod audit;

pub(super) const REQUEST_KIND: &str = "devex-clone-identity-verification-session-reconcile-request";
const RECEIPT_STATUS: &str = "identity_verification_session_reconciled";
const INTENT: &str = "identity-session-reconciliation-intent.json";
const HTTP: &str = "identity-session-http.json";

pub(super) fn execute(
    guard: &Guard<'_, '_>,
    seed: &Value,
    binding: &Value,
    request: &Value,
) -> Result<Value> {
    let context = guard.live.context;
    let scope = Settlement::load(guard, seed, binding, request)?;
    let schedules = guard.full()?;
    let check = || {
        scope.inputs.check()?;
        guard.quick(&schedules)
    };
    check()?;
    let target = guard.live.target();
    let (selected, _) = selection::transition(
        context.backend,
        &context.target,
        Some(context.storage),
        Some(context.owner),
    )?;
    let redis = &selected.value["redis"];
    if *redis != scope.redis {
        return Err("原 Redis 进程或代次与已确认会话前像不同".into());
    }
    let subject = scope.subject()?;
    let (action, intent) = start_or_observe(guard, &scope, &target, &subject, &check)?;
    if action["request_started"] != true
        || action["admin_login_confirmed"] != true
        || action["admin_session_closed"] != true
    {
        return Err("必须先核对原请求边界及该次管理员会话收口，禁止重发".into());
    }
    let sid = document::text(&intent["sid"], "原私有会话标识")?;
    let after = identity_session_redis::observe(&target, redis, &subject, Some(sid))?;
    if !after.cleared {
        return Err("原会话或其索引后像仍未收口，禁止重发".into());
    }
    let audit = audit::confirm(
        &target,
        document::text(
            &context.post.request["source_admin"]["username"],
            "受管管理员",
        )?,
        sid,
        action["request_id"].as_str(),
        document::text(&intent["started_at"], "原操作窗口")?,
        &check,
    )?;
    check()?;
    if guard.full()? != schedules {
        return Err("会话收口期间原停用调度证明变化".into());
    }
    let final_read = identity_session_redis::observe(&target, redis, &subject, Some(sid))?;
    if !final_read.cleared {
        return Err("原会话在收尾期间再次出现".into());
    }
    scope.inputs.check()?;
    let evidence = context.output.join("identity-session-reconciliation.json");
    document::publish_new(
        &evidence,
        &json!({"format_version":1,"status":RECEIPT_STATUS,"request":scope.binding,
            "failed_verification":scope.failed,"api_publication":scope.api,
            "before":scope.before,"after":after.proof,"final_observation":final_read.proof,
            "http":action,"audit":audit,"restore_qualified":false}),
        "原身份验证遗留会话收口",
    )?;
    Ok(
        json!({"status":RECEIPT_STATUS,"failed_verification":scope.failed,
        "api_publication":scope.api,"session_receipt":common::binding(&evidence)?,
        "restore_qualified":false}),
    )
}

struct Settlement {
    binding: Value,
    failed: Value,
    api: Value,
    before: Value,
    inputs: Inputs,
    plan: Value,
    redis: Value,
    user: Value,
    user_id: i64,
    login_epoch: i64,
    sid_hash: String,
}

impl Settlement {
    fn load(guard: &Guard<'_, '_>, seed: &Value, binding: &Value, request: &Value) -> Result<Self> {
        let context = guard.live.context;
        let (backend, run) = (context.backend, context.run);
        document::exact(
            request,
            &[
                "format_version",
                "kind",
                "failed_verification",
                "api_publication",
                "before",
            ],
            "身份验证会话和解请求",
        )?;
        if request["format_version"] != 1 || request["kind"] != REQUEST_KIND {
            return Err("身份验证会话和解请求类型不同".into());
        }
        let failed = request["failed_verification"].clone();
        let api = request["api_publication"].clone();
        let before = request["before"].clone();
        let before_image = common::read_bound(backend, &before, "原权限会话前像")?;
        let saved = state::load(run, true)?;
        let attempts = state::attempts(&saved)?;
        let (failed_number, failed_path) = original_failure(backend, run, attempts, &failed)?;
        let selected = api_successor::select(backend, run, &context.post.descriptor)?
            .ok_or("缺少同一原账本的新 API 后继")?;
        if selected.binding != api
            || selected.passed_attempt <= failed_number
            || selected.passed_attempt >= context.number
        {
            return Err("会话收口必须绑定原失败之后已通过的新 API".into());
        }
        let old_tool_path = super::super::seed::identity_verification_tool::path(run);
        let old_tool = common::binding(&old_tool_path)?;
        let old = common::read_bound(backend, &old_tool, "原身份核验登记")?;
        if old["first_attempt"] != failed_number || old["identity_plan"] != seed["identity_plan"] {
            return Err("旧会话与原稳定身份计划绑定不同".into());
        }
        let plan = common::read_bound(backend, &seed["identity_plan"], "原身份计划")?;
        let ledger_path = PathBuf::from(document::text(&seed["identity_state"], "原身份账本")?)
            .join("ledger.json");
        let ledger_binding = common::binding(&ledger_path)?;
        let ledger = common::read_bound(backend, &ledger_binding, "原 verifying 身份账本")?;
        if ledger["status"] != "verifying"
            || ledger["plan_sha256"] != plan["plan_sha256"]
            || !same_sha(
                &before_image["IdentityLedgerSha"],
                &ledger_binding["sha256"],
            )
        {
            return Err("旧会话前像与原 verifying 身份账本不同".into());
        }
        let (redis, marker_path) = markers(backend, attempts, failed_number, &before_image)?;
        let group = &plan["groups"][0];
        let user = group["users"][15].clone();
        let slot = document::text(&user["user_slot"], "原会话所属身份槽")?;
        if slot != "system-user-016" || group["tenant_id"] != "system" {
            return Err("前像未绑定唯一批准的系统主体".into());
        }
        let user_id: i64 = document::text(&ledger["users"][slot], "原用户 ID")?.parse()?;
        let (sid_hash, login_epoch) = prior_subject(&before_image)?;
        let inputs = Inputs::open([
            common::bound_file(backend, binding, "原会话和解请求")?,
            common::bound_file(backend, &before, "原会话前像")?,
            common::bound_file(backend, &api, "活动 API 发布")?,
            common::bound_file(backend, &seed["identity_plan"], "原身份计划")?,
            ledger_path,
            old_tool_path,
            failed_path,
            marker_path,
        ])?;
        Ok(Self {
            binding: binding.clone(),
            failed,
            api,
            before,
            inputs,
            plan,
            redis,
            user,
            user_id,
            login_epoch,
            sid_hash,
        })
    }

    fn subject(&self) -> Result<Subject<'_>> {
        Ok(Subject {
            scope: document::text(&self.plan["environment"]["scope_id"], "原身份 scope")?,
            tenant: "system",
            user_id: self.user_id,
            username: document::text(&self.user["username"], "原用户")?,
            client_address: document::text(&self.user["client_address"], "原用户测试地址")?,
            login_epoch: self.login_epoch,
            sid_sha256: &self.sid_hash,
        })
    }
}

fn original_failure(
    backend: &Path,
    run: &Path,
    attempts: &[Value],
    failed: &Value,
) -> Result<(u64, PathBuf)> {
    let row = attempts
        .iter()
        .rev()
        .find(|row| row["stage"] == "seed-runtime" && row["mode"] == "identities-verify")
        .filter(|row| row["status"] == "failed" && row["result"].is_null())
        .ok_or("缺少最近一次未通过的原身份核验")?;
    let number = row["number"].as_u64().ok_or("原身份核验序号无效")?;
    let path = run.join(format!("failure-{number:04}.json"));
    if common::bound_file(backend, failed, "原身份核验失败")? != path {
        return Err("原身份核验失败文件日期或序号不同".into());
    }
    common::read_bound(backend, failed, "原身份核验失败")?;
    Ok((number, path))
}

fn markers(
    backend: &Path,
    attempts: &[Value],
    failed: u64,
    before: &Value,
) -> Result<(Value, PathBuf)> {
    let row = attempts
        .iter()
        .rev()
        .find(|row| {
            row["stage"] == "target-cache-markers"
                && row["number"].as_u64().is_some_and(|number| number < failed)
        })
        .filter(|row| row["status"] == "passed")
        .ok_or("原会话缺少已确认的 Redis 标记")?;
    let result = common::read_bound(backend, &row["result"], "原 Redis 标记结果")?;
    let binding = &result["observation"];
    let marker = common::read_bound(backend, binding, "原 Redis 标记观察")?;
    if !same_sha(&before["OriginalMarkerProofSha"], &binding["sha256"])
        || marker["confirmed"] != true
        || marker["classification"] != "after"
    {
        return Err("前像未绑定原 Redis 归属证明".into());
    }
    Ok((
        marker["binding"]["fixture"]["redis"].clone(),
        common::bound_file(backend, binding, "原 Redis 标记观察")?,
    ))
}

fn prior_subject(before: &Value) -> Result<(String, i64)> {
    let entries = document::array(&before["MatchingRows"], "遗留会话前像")?;
    let [only] = entries else {
        return Err("前像未证明唯一遗留会话".into());
    };
    if before["ResourceIdentity"] != "same_boot_registered_redis"
        || before["RemoteWrites"] != 0
        || only["LiveFamily"] != true
        || only["MetadataAddressMatches"] != true
        || only["MetadataIdentityMatches"] != true
        || only["FamilyIdentityMatches"] != true
        || only["MatchesLastLoginTime"] != true
        || only["FromRefreshUserIndex"] != true
        || only["FromOnlineUserIndex"] != true
        || only["FromRefreshTenantIndex"] != 1
        || only["FromOnlineTenantIndex"] != 1
    {
        return Err("原会话前像的身份或索引未同时确认".into());
    }
    let digest = document::text(&only["SidSha"], "原会话摘要")?.to_ascii_lowercase();
    let epoch = only["LoginEpoch"].as_i64().ok_or("原会话登录时刻无效")?;
    Ok((digest, epoch))
}

fn same_sha(left: &Value, right: &Value) -> bool {
    matches!((left.as_str(),right.as_str()), (Some(a),Some(b)) if a.eq_ignore_ascii_case(b))
}

fn start_or_observe(
    guard: &Guard<'_, '_>,
    scope: &Settlement,
    target: &StorageInputs<'_>,
    subject: &Subject<'_>,
    check: &dyn Fn() -> Result<()>,
) -> Result<(Value, Value)> {
    let context = guard.live.context;
    let path = context.run.join("seed-runtime").join(INTENT);
    if path.try_exists()? {
        return observe_prior_action(context, scope, target, subject, &path, check);
    }
    let before = identity_session_redis::observe(target, &scope.redis, subject, None)?;
    if !before.active {
        return Err("原会话没有确认活跃前像，拒绝调用强退".into());
    }
    let idempotency_key = uuid::Uuid::new_v4().to_string();
    let started = OffsetDateTime::now_utc().format(&Rfc3339)?;
    let intent = json!({"format_version":1,"kind":"devex-seed-identity-session-reconciliation-intent",
        "request":scope.binding,"api_publication":scope.api,"first_attempt":context.number,
        "sid":before.sid,"sid_sha256":scope.sid_hash,"idempotency_key":idempotency_key,"started_at":started});
    check()?;
    document::publish_new(&path, &intent, "原会话唯一强退意图")?;
    let before_path = context.output.join("identity-session-before.json");
    document::publish_new(&before_path, &before.proof, "原会话强退前像")?;
    let intent_binding = common::binding(&path)?;
    let state = context.run.join("state.json");
    let owner = context.run.join("run.lock/owner.json");
    let before_bindings = [
        intent_binding,
        common::binding(&state)?,
        common::binding(&owner)?,
        scope.binding.clone(),
        scope.api.clone(),
        common::binding(&before_path)?,
    ];
    let backend = context.backend.to_path_buf();
    let request_guard = move || {
        for binding in &before_bindings {
            common::bound_file(&backend, binding, "强退发送前受管绑定")?;
        }
        Ok(())
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let outcome = runtime.block_on(force_logout_as_system_admin(
        AdminForceLogout {
            coordinator: context.backend,
            run: context.run,
            attempt: context.number,
            plan: &scope.plan,
            artifacts: context.output.join("old-session-administrator"),
            administrator: &context.post.request["source_admin"],
            sid: document::text(&intent["sid"], "原私有会话标识")?,
            idempotency_key: &idempotency_key,
        },
        identity_session_reconciliation_api_proof,
        request_guard,
    ))?;
    let action = http_evidence(&outcome);
    document::publish_new(&context.output.join(HTTP), &action, "原会话退出 HTTP 结果")?;
    check()?;
    Ok((action, intent))
}

fn observe_prior_action(
    context: &super::context::Context<'_>,
    scope: &Settlement,
    target: &StorageInputs<'_>,
    subject: &Subject<'_>,
    path: &Path,
    check: &dyn Fn() -> Result<()>,
) -> Result<(Value, Value)> {
    let binding = common::binding(path)?;
    let intent = common::read_bound(context.backend, &binding, "原会话强退意图")?;
    let number = intent["first_attempt"]
        .as_u64()
        .ok_or("原强退意图缺少序号")?;
    let saved = state::load(context.run, true)?;
    let previous = state::attempts(&saved)?.iter().any(|row| {
        row["number"] == number
            && number < context.number
            && row["stage"] == "seed-runtime"
            && row["mode"] == "identities-reconcile"
            && row["status"] == "failed"
    });
    if intent["format_version"] != 1
        || intent["kind"] != "devex-seed-identity-session-reconciliation-intent"
        || intent["request"] != scope.binding
        || intent["api_publication"] != scope.api
        || intent["sid_sha256"] != scope.sid_hash
        || !previous
    {
        return Err("已有会话强退意图不属于本次原请求与失败阶段，禁止重发".into());
    }
    let prior = context
        .run
        .join("seed-runtime")
        .join(format!("attempt-{number:04}"))
        .join(HTTP);
    let http = common::binding(&prior)?;
    check()?;
    let live = identity_session_redis::observe(
        target,
        &scope.redis,
        subject,
        Some(document::text(&intent["sid"], "原私有会话标识")?),
    )?;
    let action = common::read_bound(context.backend, &http, "原强退响应与收口证明")?;
    if !live.cleared {
        return Err("原强退有未知结果且会话后像未收口，禁止重发".into());
    }
    common::bound_file(context.backend, &binding, "原强退意图")?;
    Ok((action, intent))
}

fn http_evidence(outcome: &AdminForceLogoutOutcome) -> Value {
    json!({"admin_login_confirmed":outcome.admin_login_confirmed,
        "request_started":outcome.request_started,"status":outcome.status,
        "request_id":outcome.request_id,"operation_confirmed":outcome.operation_confirmed,
        "admin_session_closed":outcome.admin_session_closed,"failure":outcome.failure})
}

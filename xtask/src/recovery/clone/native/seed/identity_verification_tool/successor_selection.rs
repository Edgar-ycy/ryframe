use super::*;

pub(crate) fn select(
    backend: &Path,
    run: &Path,
    saved: &Value,
    supplied: Option<&Path>,
    active: &api_successor::Selected,
    current: Current<'_>,
) -> Result<(Value, Value)> {
    let number = number_of(&current.row["number"])?;
    let explicit = supplied
        .map(|path| common::binding(&common::common_file(backend, path, "身份核验后继请求")?))
        .transpose()?;
    let explicit_request = explicit
        .as_ref()
        .map(|binding| common::read_bound(backend, binding, "身份核验后继请求"))
        .transpose()?;
    let kind = match explicit_request
        .as_ref()
        .and_then(|value| value["kind"].as_str())
    {
        Some(RECOVERY_REQUEST) => Kind::Recovery,
        Some(RECOVERY_RETRY_REQUEST) => Kind::RecoveryRetry,
        _ => Kind::Canonical,
    };
    let filename = kind.path(run, number);
    let existing = match fs::symlink_metadata(&filename) {
        Ok(_) => {
            let binding = common::binding(&filename)?;
            let record = read_at(backend, &binding, &filename)?;
            require_attempt(&record, number)?;
            Some((binding, record))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let request = match (&existing, explicit) {
        (None, Some(request)) => request,
        (None, None) => return Err("首次登记第二代 API 的独立核验工具需要显式请求".into()),
        (Some((_, record)), supplied) => {
            if supplied
                .as_ref()
                .is_some_and(|binding| *binding != record["request"])
            {
                return Err("已有独立核验工具后继只接受原请求".into());
            }
            record["request"].clone()
        }
    };
    let authority = Authority::load(
        backend,
        run,
        state::attempts(saved)?,
        number,
        active,
        &request,
        kind,
    )?;
    let history = &authority.legacy.history;
    if history.original != *current.original || history.request != *current.request {
        return Err("独立核验工具后继未保留原 seed 登记和准备账本".into());
    }
    history.check_departments(current.departments)?;
    authority.check_verifying(backend)?;
    let (controller, owner) = verify_controller(run, current.row, true)?;
    if owner != *current.owner {
        return Err("独立核验工具后继的当前控制器已变化".into());
    }
    let protected = authority.protect(backend)?;
    let (expected, tool) = authority.record(backend, number, &controller, &owner)?;
    protected.check()?;
    let binding = match existing {
        Some((binding, previous)) if previous == expected => binding,
        Some(_) => return Err("已有独立核验工具后继与原不可变授权不同".into()),
        None => {
            document::publish_new(&filename, &expected, "独立身份核验工具后继")?;
            common::binding(&filename)?
        }
    };
    if read_at(backend, &binding, &filename)? != expected || state::load(run, true)? != *saved {
        return Err("独立核验工具后继登记或正式阶段在读回期间变化".into());
    }
    protected.check()?;
    Ok((authority.request_with_tool(tool), binding))
}

pub(crate) fn resolve(
    backend: &Path,
    run: &Path,
    descriptor: &Value,
    number: Option<u64>,
    cleanup: bool,
) -> Result<Value> {
    let number = number.ok_or("独立核验工具后继缺少正式阶段编号")?;
    let path = common::bound_file(backend, descriptor, "身份核验后继登记")?;
    let kind = if path == recovery_retry_path(run, number) {
        Kind::RecoveryRetry
    } else if path == recovery_path(run) {
        Kind::Recovery
    } else {
        Kind::Canonical
    };
    let filename = kind.path(run, number);
    let record = read_at(backend, descriptor, &filename)?;
    require_attempt(&record, number)?;
    let saved = state::load(run, true)?;
    let attempts = state::attempts(&saved)?;
    let row = verify_attempt(attempts, number)?;
    let (controller, owner) = verify_controller(run, row, !cleanup)?;
    let original = read_at(
        backend,
        &record["original_registration"],
        &run.join("seed-runtime.json"),
    )?;
    let active = if cleanup {
        api_successor::select_before(backend, run, &original["post_copy"], number)?
    } else {
        api_successor::select(backend, run, &original["post_copy"])?
    }
    .ok_or("独立核验工具后继缺少本阶段之前的 API 发布")?;
    let authority = Authority::load(
        backend,
        run,
        attempts,
        number,
        &active,
        &record["request"],
        kind,
    )?;
    let protected = authority.protect(backend)?;
    let (expected, tool) = authority.record(backend, number, &controller, &owner)?;
    if record != expected {
        return Err("独立核验工具后继不同于原登记、会话和解或冻结构建".into());
    }
    if !cleanup {
        authority.check_verifying(backend)?;
        let history = &authority.legacy.history;
        history.check_departments(&department::authorize_identity_verification(
            backend,
            run,
            &history.request,
            number,
        )?)?;
    }
    protected.check()?;
    if read_at(backend, descriptor, &filename)? != record || state::load(run, true)? != saved {
        return Err("独立核验工具后继在解析期间变化".into());
    }
    Ok(authority.request_with_tool(tool))
}

pub(super) struct PreviousRecovery {
    pub(super) descriptor: Value,
    pub(super) tool: Value,
    request: Value,
    build: Value,
}

impl PreviousRecovery {
    pub(super) fn protected(&self) -> [&Value; 3] {
        [&self.descriptor, &self.request, &self.build]
    }
}

pub(super) fn previous_recovery(
    backend: &Path,
    run: &Path,
    rows: &[Value],
    number: u64,
    supplied: &Value,
    authority: &Authority,
) -> Result<PreviousRecovery> {
    let descriptor = &supplied["previous_recovery"];
    let previous = previous_recovery_attempt(
        backend,
        run,
        rows,
        number,
        descriptor,
        &authority.prelaunch_failures,
    )?;
    let request = &previous.record["request"];
    let original = request_value(backend, request, &authority.predecessor, Kind::Recovery)?;
    if ["settlement_result", "recovered_result", "legacy_source"]
        .iter()
        .any(|key| original[*key] != supplied[*key])
    {
        return Err("已登记的恢复核验重试必须沿用原会话和解及生产者退场证明".into());
    }
    let build = &original["build"];
    let tool = continuation_tool::verify(backend, build, PURPOSE, &previous.owner)?;
    let mut expected = authority.record_value(previous.number, &previous.controller, &tool);
    expected["request"] = request.clone();
    expected["build"] = build.clone();
    expected["prelaunch_failures"] = json!(&authority.prelaunch_failures[2..]);
    if expected != previous.record {
        return Err("原固定恢复登记不同于原失败控制器、会话和解、账本或工具".into());
    }
    Ok(PreviousRecovery {
        descriptor: descriptor.clone(),
        tool,
        request: request.clone(),
        build: build.clone(),
    })
}

struct PreviousRecoveryAttempt {
    number: u64,
    record: Value,
    controller: Value,
    owner: Value,
}

fn previous_recovery_attempt(
    backend: &Path,
    run: &Path,
    rows: &[Value],
    number: u64,
    descriptor: &Value,
    prelaunch: &[Value],
) -> Result<PreviousRecoveryAttempt> {
    let record = read_at(backend, descriptor, &recovery_path(run))?;
    let prior = number_of(&record["first_attempt"])?;
    if prior.checked_add(1) != Some(number) {
        return Err("已登记的恢复核验只允许接续紧邻的下一阶段".into());
    }
    let row = verify_attempt(rows, prior)?;
    let receipt = common::binding(&run.join(format!("failure-{prior:04}.json")))?;
    let controller = failure_controller(backend, run, row, &receipt)?;
    post_process::files::absent(&output(run, prior).join("identity-result.json"))?;
    if record["controller"] != controller || !prelaunch.starts_with(&[receipt, controller.clone()])
    {
        return Err("原固定恢复登记缺少本阶段已关闭且未启动的失败和控制器证明".into());
    }
    let (_, owner) = verify_controller(run, row, false)?;
    Ok(PreviousRecoveryAttempt {
        number: prior,
        record,
        controller,
        owner,
    })
}

pub(super) fn recovery_evidence(
    backend: &Path,
    run: &Path,
    rows: &[Value],
    number: u64,
    supplied: &Value,
    settlement: &Settlement,
    api_publication: &Value,
) -> Result<Recovered> {
    let file = path(run);
    let previous = common::binding(&file)?;
    let record = read_at(backend, &previous, &file)?;
    let evidence = recovered_producer::require(
        backend,
        run,
        rows,
        number,
        &previous,
        &supplied["recovered_result"],
        &supplied["legacy_source"],
    )?;
    if record["first_attempt"] != evidence.attempt
        || evidence.attempt <= settlement.number
        || record["predecessor"] != supplied["predecessor"]
        || record["settlement_result"] != supplied["settlement_result"]
        || record["api_publication"] != *api_publication
        || record["verifying_ledger_sha256"] != settlement.ledger_sha256
        || !digest_shape(&record["tool"]["sha256"])
    {
        return Err("已收口核验与原会话和解及 verifying 账本不一致".into());
    }
    Ok(Recovered {
        evidence,
        tool_sha256: record["tool"]["sha256"].clone(),
    })
}

pub(super) fn recovered_file(descriptor: &Value) -> Result<PathBuf> {
    use crate::local_test_path::{LocalTestPathKind, validate_absolute_path};
    manifest::validate_binding_shape(descriptor, "已收口核验的固定证据")?;
    let path = PathBuf::from(document::text(&descriptor["path"], "已收口核验的证据路径")?);
    validate_absolute_path(&path, LocalTestPathKind::ExistingFile)?;
    if common::binding(&path)? != *descriptor {
        return Err("已收口核验的固定证据与登记摘要不一致".into());
    }
    Ok(path)
}

#[cfg(test)]
#[test]
fn retry_requires_closed_prelaunch_failure_after_settlement() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let backend = fixture.path();
    let run = backend.join(".local-tests/run");
    let stage = output(&run, 69);
    fs::create_dir_all(&run)?;
    document::write_new(&run.join("manifest.json"), &json!({}))?;
    let mut row = json!({"number":69,"stage":"seed-runtime","mode":MODE,
        "status":"running","finished_at":null,"result":null,"error_type":null});
    let controller = state::controller_path(&run, 69);
    let owner = json!({"format_version":1,"directory":manifest::path_text(&run, "夹具运行目录")?,
        "identity":{"pid":2,"started":"1","executable":run.join("tool.exe")},
        "manifest_sha256":state::manifest_binding(&run)?["sha256"]});
    document::write_new(
        &controller,
        &json!({"format_version":1,"kind":"devex-stage-controller",
        "owner":owner,"attempt":69,"attempt_sha256":crate::recovery::clone::native::digest::plan_hash(&row)?}),
    )?;
    document::write_new(
        &run.join("failure-0069.json"),
        &json!({"format_version":2,
        "kind":"devex-stage-failure","attempt":69,"stage":"seed-runtime","mode":MODE,
        "error_type":"StageError","operation_id":"clone.seed-runtime.identities-verify",
        "controller":common::binding(&controller)?}),
    )?;
    assert!(prelaunch_failures(backend, &run, [&row], 68).is_err());
    row["status"] = json!("failed");
    row["finished_at"] = json!("2026-11-27T01:33:57Z");
    row["error_type"] = json!("StageError");
    let checked = prelaunch_failures(backend, &run, [&row], 68)?;
    assert_eq!(checked.len(), 2);
    assert!(prelaunch_failures(backend, &run, [&row], 69).is_err());

    fs::create_dir_all(&stage)?;
    let original = recovery_path(&run);
    let record = json!({"first_attempt":69,"controller":common::binding(&controller)?});
    document::write_new(&original, &record)?;
    let binding = common::binding(&original)?;
    let rows = std::slice::from_ref(&row);
    let previous = previous_recovery_attempt(backend, &run, rows, 70, &binding, &checked)?;
    assert_eq!(previous.record, record);
    assert_eq!(common::binding(&original)?, binding);
    assert_ne!(Kind::RecoveryRetry.path(&run, 70), original);
    for number in [69, 71] {
        assert!(
            previous_recovery_attempt(backend, &run, rows, number, &binding, &checked).is_err()
        );
    }
    assert!(previous_recovery_attempt(backend, &run, rows, 70, &binding, &checked[1..]).is_err());
    assert!(previous_recovery_attempt(backend, &run, rows, 70, &checked[0], &checked).is_err());

    for name in [
        "session-launch.json",
        "session-process.json",
        "session.stdout.log",
        "session.stderr.log",
    ] {
        let conflicting = stage.join(name);
        document::write_new(&conflicting, &json!({}))?;
        assert!(
            prelaunch_failures(backend, &run, [&row], 68).is_err(),
            "{name}"
        );
        fs::remove_file(conflicting)?;
    }
    assert_eq!(common::binding(&original)?, binding);
    Ok(())
}

#[cfg(test)]
#[test]
fn successor_request_rejects_a_changed_predecessor_or_reused_attempt() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let backend = fixture.path();
    let run = backend.join(".local-tests/run");
    fs::create_dir_all(run.join("seed-runtime"))?;
    let predecessor_path = super::super::path(&run);
    document::write_new(&predecessor_path, &json!({"first_attempt":65}))?;
    let predecessor = common::binding(&predecessor_path)?;
    let input = run.join("verification-successor-request.json");
    document::write_new(
        &input,
        &json!({"format_version":1,
        "kind":"devex-seed-identity-verification-successor-request","predecessor":predecessor,
        "settlement_result":{"attempt":67},"build":{"fixture":true}}),
    )?;
    let binding = common::binding(&input)?;
    assert_eq!(
        request_value(backend, &binding, &predecessor, Kind::Canonical)?["predecessor"],
        predecessor
    );
    assert!(
        request_value(
            backend,
            &binding,
            &json!({"different":true}),
            Kind::Canonical
        )
        .is_err()
    );
    assert!(request_value(backend, &binding, &predecessor, Kind::Recovery).is_err());

    let input = run.join("verification-recovery-request.json");
    document::write_new(
        &input,
        &json!({"format_version":1,"kind":RECOVERY_REQUEST,
        "predecessor":predecessor,"settlement_result":{"attempt":68},"build":{"fixture":true},
        "recovered_result":{"attempt":71},"legacy_source":{"fixture":true}}),
    )?;
    let binding = common::binding(&input)?;
    assert!(request_value(backend, &binding, &predecessor, Kind::Canonical).is_err());
    assert_eq!(
        request_value(backend, &binding, &predecessor, Kind::Recovery)?["recovered_result"]["attempt"],
        71
    );

    let input = run.join("verification-recovery-retry-request.json");
    let mut retry = request_value(backend, &binding, &predecessor, Kind::Recovery)?;
    retry["kind"] = json!(RECOVERY_RETRY_REQUEST);
    document::write_new(&input, &retry)?;
    let missing_previous = common::binding(&input)?;
    assert!(
        request_value(
            backend,
            &missing_previous,
            &predecessor,
            Kind::RecoveryRetry
        )
        .is_err()
    );

    let valid = run.join("verification-recovery-retry-with-previous.json");
    let previous = recovery_path(&run);
    document::write_new(&previous, &json!({"first_attempt":73}))?;
    retry["previous_recovery"] = common::binding(&previous)?;
    document::write_new(&valid, &retry)?;
    let selected = common::binding(&valid)?;
    assert_eq!(
        request_value(backend, &selected, &predecessor, Kind::RecoveryRetry)?,
        retry
    );
    assert!(request_value(backend, &selected, &predecessor, Kind::Recovery).is_err());

    let record = json!({"first_attempt":68});
    require_attempt(&record, 68)?;
    assert!(require_attempt(&record, 65).is_err());
    assert!(require_attempt(&record, 69).is_err());
    Ok(())
}

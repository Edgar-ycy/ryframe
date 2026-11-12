use super::*;
use crate::recovery::{
    clone::native::{
        lifecycle::{identity, lock::ProcessGuard},
        target_verify::authority::require_closed,
    },
    runtime::REGISTRATION_GUARD,
    source::evidence::MARKER,
};
use std::process::Stdio;

#[test]
fn recover_completes_first_directory_cut_without_starting_roles() -> Result<()> {
    let mut fixture = Fixture::new();
    fixture.root = fixture.control.join("case").join("runtime");
    let result = (|| {
        let program = native_coordinator()?;
        drop(configure_with_artifact(
            &mut fixture,
            &program,
            &program,
            None,
        )?);
        let (owner, owner_path, intent_binding) = interrupted_start(&fixture, &program)?;
        let intent = document::read(
            &generation::creation_path(&fixture.root),
            &fixture.backend,
            "原首次目录创建意图",
        )?;
        check(
            !fixture.root.try_exists()? && document::binding(&intent) == intent_binding,
            "隔离恢复夹具未停在原意图已发布、运行根尚未创建的切点",
        )?;

        let result = recover(&fixture, &program, &owner_path)?;
        verify_first_generation(&fixture, &result, &intent, &intent_binding)?;
        verify_owner_archive(&fixture, &result, &owner)?;
        check(
            !owner_path.try_exists()?
                && !fixture.control.join(REGISTRATION_LOCK).try_exists()?
                && fixture.control.join(REGISTRATION_GUARD).is_file(),
            "公开 Recover 未释放原控制锁或丢失内核 guard",
        )?;
        ProcessGuard::acquire_named(&fixture.control, REGISTRATION_GUARD)?.check()
    })();
    if let Err(error) = result {
        let path = fixture._work.keep();
        return Err(format!("{error}；首次目录恢复测试证据：{}", path.display()).into());
    }
    Ok(())
}

fn interrupted_start(fixture: &Fixture, program: &Path) -> Result<(Value, PathBuf, Value)> {
    let _guard = ProcessGuard::acquire_named(&fixture.control, REGISTRATION_GUARD)?;
    let operation = start_operation(&fixture.binding)?;
    // 原生子进程等待空 stdin，提供实际创建身份，不触发来源操作或产品启动。
    let mut child = crate::process::child_command(program)
        .current_dir(&fixture.backend)
        .arg(MARKER)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let published: Result<_> = (|| {
        let process = identity::process(child.id())?.ok_or("原控制器夹具未保持运行")?;
        let owner = json!({
            "format_version":1,"kind":"restore-runtime-registration-controller",
            "identity":process,"runtime_directory":fixture.control,"operation":operation,
            "token":uuid::Uuid::new_v4().simple().to_string(),
        });
        let lock = fixture.control.join(REGISTRATION_LOCK);
        fs::create_dir(&lock)?;
        let owner_path = lock.join("owner.json");
        strict::write_new(&owner_path, &owner)?;
        let intent = fixture.runtime().model(
            "create-intent",
            &fixture.root,
            &fixture.request,
            &Value::Null,
            &fixture.commands,
            1,
        )?;
        Ok((owner, owner_path, intent))
    })();
    drop(child.stdin.take());
    check(
        child.wait()?.code() == Some(1),
        "原控制器夹具未按空输入退出",
    )?;
    let (owner, owner_path, intent) = published?;
    stop::require_exited(&owner["identity"])?;
    Ok((owner, owner_path, intent))
}

fn recover(fixture: &Fixture, program: &Path, owner: &Path) -> Result<Value> {
    let result = crate::process::child_command(program)
        .current_dir(&fixture.backend)
        .args([
            "check",
            "recovery",
            "runtime",
            "recover",
            "--runtime-registration",
        ])
        .arg(fixture.control.join("registration.json"))
        .arg("--target-plan")
        .arg(fixture.control.join("target.json"))
        .arg("--owner")
        .arg(owner)
        .args(["--generation", "1", "--write"])
        .output()?;
    check(
        result.status.success(),
        &String::from_utf8_lossy(&result.stderr),
    )?;
    strict::parse_fragment(std::str::from_utf8(&result.stdout)?, "公开 Recover 结果")
}

fn verify_first_generation(
    fixture: &Fixture,
    result: &Value,
    intent: &Document,
    intent_binding: &Value,
) -> Result<()> {
    let directory = fixture.root.join("generation-0001");
    check(
        result["operation"] == "recover"
            && result["status"] == "stopped"
            && result["generation"] == 1
            && result["runtime_directory"] == json!(directory),
        "公开 Recover 未停止同一首代运行",
    )?;
    let saved = strict::read(&fixture.root.join(STATE))?;
    let generations = strict::array(&saved["generations"], "已恢复首代生命周期")?;
    check(
        saved["creation_intent"] == *intent_binding
            && file_binding(fixture, &intent.path)? == *intent_binding
            && generations.len() == 1,
        "Recover 改写了原创建意图或引入了额外代次",
    )?;
    intent.assert_unchanged()?;
    let first = &generations[0];
    check(
        first["number"] == 1
            && first["status"] == "stopped"
            && first["request"] == fixture.request
            && first["directory"] == json!(directory)
            && first["launch"].is_null()
            && fs::read_dir(&directory)?.next().is_none(),
        "首代未保留原启动请求、最终停止状态或空代次目录",
    )?;
    for role in ROLES {
        let item = &first["roles"][role];
        let observed = &result["processes"][role];
        check(
            item["state"] == "stopped"
                && item["command"] == fixture.commands[role]
                && item["tree"].is_null()
                && item["completion"].is_null()
                && *observed
                    == json!({"state":"pending","identity":null,"tree":null,"completion":null}),
            "Recover 为从未启动的角色创建了进程或关闭证明",
        )?;
        let endpoint = &fixture.request["authority"][format!("{role}_endpoint")];
        require_closed(document::text(endpoint, "恢复后原登记端点")?)?;
    }
    Ok(())
}

fn verify_owner_archive(fixture: &Fixture, result: &Value, owner: &Value) -> Result<()> {
    let archive = PathBuf::from(document::text(
        &result["recovery"]["receipt"],
        "原控制器恢复归档路径",
    )?);
    check(
        archive.parent() == Some(fixture.control.as_path())
            && archive.file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .starts_with("restore-registration-reconcile-")
            }),
        "控制器归档不在原登记控制目录",
    )?;
    let expected = json!({
        "format_version":1,"kind":"restore-runtime-registration-reconciliation",
        "owner":owner,"observed_identity":null,"remote_writes_reconciled":false,
    });
    check(
        strict::read(&archive)? == expected
            && result["recovery"]["owner"] == *owner
            && result["recovery"]["observed_identity"].is_null()
            && result["recovery"]["remote_writes_reconciled"] == false,
        "Recover 未归档原控制器创建身份或错误声明远端写入已核对",
    )
}

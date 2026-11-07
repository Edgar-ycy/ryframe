//! 在独立 reconcile 阶段收尾已正常退出的原 Redis；绝不重发 cache-stop。
use super::*;
use crate::recovery::clone::native::cache::host_control::recovery_port_error;

pub(super) struct TreeProof {
    members: PathBuf,
    files: Vec<TreeFile>,
}
struct TreeFile {
    path: PathBuf,
    identity: same_file::Handle,
    binding: Value,
    modified: std::time::SystemTime,
}
impl TreeProof {
    fn capture(services: &Value) -> Result<Self> {
        let tree = &services["tree"];
        let root = Path::new(text(tree, "runtime_directory")?);
        let operation = text(tree, "operation_id")?;
        let members = root.join(format!("rustfs-members-{operation}-stopped.json"));
        let paths = [
            root.join("rustfs-tree.json"),
            members.clone(),
            root.join(format!("rustfs-tree-{operation}-control.json")),
            root.join(format!("rustfs-tree-{operation}-result.json")),
        ];
        for entry in std::fs::read_dir(root)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_str().ok_or("原RustFS记录名不是UTF-8")?;
            if name.starts_with("rustfs-tree-")
                && (name.ends_with("-control.json") || name.ends_with("-result.json"))
            {
                check(
                    paths.contains(&entry.path()),
                    "原RustFS目录存在无法归属的控制或完成记录",
                )?;
            }
        }
        let mut files = Vec::new();
        for path in paths {
            let metadata = match std::fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound
                        && path != root.join("rustfs-tree.json") =>
                {
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            check(metadata.is_file(), "关闭恢复原树证据不是普通文件")?;
            let identity = same_file::Handle::from_path(&path)?;
            let descriptor = binding(&path)?;
            check(
                identity == same_file::Handle::from_path(&path)?
                    && metadata.modified()? == std::fs::metadata(&path)?.modified()?,
                "关闭恢复原树证据在冻结期间变化",
            )?;
            files.push(TreeFile {
                path,
                identity,
                binding: descriptor,
                modified: metadata.modified()?,
            });
        }
        Ok(Self { members, files })
    }

    fn verify(&self, services: &Value) -> Result<()> {
        let current = Self::capture(services)?;
        check(
            self.files.len() == current.files.len()
                && self
                    .files
                    .iter()
                    .zip(&current.files)
                    .all(|(before, after)| {
                        before.path == after.path
                            && before.identity == after.identity
                            && before.binding == after.binding
                            && before.modified == after.modified
                    }),
            "关闭恢复原树证据新增、替换或漂移",
        )
    }
}

pub(super) fn run(
    runtime: &mut dyn Runtime,
    context: &Context,
    history: &Value,
    failed: &Value,
    services: &Value,
) -> Result<Value> {
    let started = runtime.now();
    let original = candidate(context, history, failed, services)?;
    let stopped = qualify(runtime, context, history, failed, services)?;
    let observed = cache_observation(runtime, services)?;
    let _guard = ProcessGuard::acquire(&context.run)?;
    owned(context, |owner| {
        context.guard(None)?;
        context.no_unknown()?;
        check(
            candidate(context, history, failed, services)? == original,
            "关闭恢复加锁前证据变化",
        )?;
        stage(
            context,
            "reconcile",
            &context.sources(),
            owner,
            |number, controller, running, output| {
                new_output(context, output)?;
                finish(
                    runtime,
                    context,
                    history,
                    failed,
                    services,
                    &original,
                    (
                        number,
                        controller,
                        running,
                        output,
                        started,
                        observed,
                        stopped.as_ref(),
                    ),
                )
            },
        )
    })
}

fn finish(
    runtime: &mut dyn Runtime,
    context: &Context,
    history: &Value,
    failed: &Value,
    services: &Value,
    original: &Value,
    stage: (
        u64,
        &Value,
        &Value,
        &Path,
        Duration,
        Value,
        Option<&TreeProof>,
    ),
) -> Result<Value> {
    let (number, controller, running, output, started, observed, stopped) = stage;
    let deadline = started + Duration::from_secs(180);
    wait_port(
        runtime,
        context,
        history,
        failed,
        services,
        original,
        (running, deadline, stopped.is_none(), stopped),
    )?;
    stable(
        runtime,
        context,
        history,
        failed,
        services,
        original,
        (running, stopped.is_none(), stopped),
    )?;
    check(
        runtime.now() < deadline,
        "关闭恢复绑定与观察完成时已过原期限",
    )?;
    if stopped.is_none() {
        runtime.call("tree-stop", json!({"tree":services["tree"],"crash":false}))?;
    }
    let completion = runtime.call("completion", json!({"tree":services["tree"]}))?;
    check(
        observe(runtime, services)? == expected_stopped(),
        "关闭恢复未完整证明成员退出及端口释放",
    )?;
    wait_port(
        runtime,
        context,
        history,
        failed,
        services,
        original,
        (running, deadline, false, stopped),
    )?;
    context.guard(Some(running))?;
    context.no_unknown()?;
    check(
        runtime.now() < deadline,
        "关闭恢复整树收尾超过本次原期限；保留已写入现场",
    )?;
    publish(
        runtime,
        context,
        history,
        failed,
        services,
        (original, observed, completion, stopped),
        (number, controller, running, output, started),
    )
}

fn wait_port(
    runtime: &mut dyn Runtime,
    context: &Context,
    history: &Value,
    failed: &Value,
    services: &Value,
    original: &Value,
    stage: (&Value, Duration, bool, Option<&TreeProof>),
) -> Result<()> {
    let (running, deadline, tree_running, stopped) = stage;
    let mut last = None;
    loop {
        if runtime.now() >= deadline {
            return Err(recovery_port_error(last.unwrap_or_else(|| {
                std::io::Error::from(std::io::ErrorKind::TimedOut).into()
            })));
        }
        stable(
            runtime,
            context,
            history,
            failed,
            services,
            original,
            (running, tree_running, stopped),
        )?;
        if runtime.now() >= deadline {
            return Err(recovery_port_error(last.unwrap_or_else(|| {
                std::io::Error::from(std::io::ErrorKind::TimedOut).into()
            })));
        }
        match runtime.call(
            "cache-closed-port",
            json!({"request":services["requests"]["redis"],"runtime":services["runtime"]}),
        ) {
            Ok(value) => {
                check(value == true, "关闭恢复缺少实际独占端口证明")?;
                break;
            }
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|io| io.kind() == std::io::ErrorKind::AddrInUse) =>
            {
                last = Some(error)
            }
            Err(error) => return Err(recovery_port_error(error)),
        }
        let remaining = deadline.saturating_sub(runtime.now());
        if remaining.is_zero() {
            return Err(recovery_port_error(last.unwrap_or_else(|| {
                std::io::Error::from(std::io::ErrorKind::TimedOut).into()
            })));
        }
        runtime.pause(remaining.min(Duration::from_millis(500)));
    }
    Ok(())
}

fn publish(
    runtime: &mut dyn Runtime,
    context: &Context,
    history: &Value,
    failed: &Value,
    services: &Value,
    evidence: (&Value, Value, Value, Option<&TreeProof>),
    stage: (u64, &Value, &Value, &Path, Duration),
) -> Result<Value> {
    let (original, observed, completion, stopped) = evidence;
    let (number, controller, running, output, started) = stage;
    let deadline = started + Duration::from_secs(180);
    let recovery_file = output.join("close-recovery.json");
    let receipt = json!({"format_version":1,"kind":"reference-fixture-close-recovery","owner":context.value["sources"]["state_before"],
        "candidate":original,"redis_observation":observed,"rustfs_tree":services["tree"],"rustfs_members":completion,
        "windows_port":{"port":services["requests"]["redis"]["port"],"bound_and_closed":true},
        "elapsed_millis":runtime.now().saturating_sub(started).as_millis(),"budget_seconds":180,"remote_writes":0,"resources_deleted":false});
    json_io::write_new(&recovery_file, &receipt)?;
    let evidence = history::evidence::recovery_evidence(
        &context.run,
        failed,
        &history["active_generation"],
        &context.sources(),
        services,
        &binding(&recovery_file)?,
    )?;
    let proof = (
        expected_stopped(),
        json!({"evidence":evidence,"failed_controller":original["failed_controller"]}),
    );
    let document = reconcile::document(context, history, failed, &proof, number, true)?;
    let file = output.join("reconciliation.json");
    json_io::write_new(&file, &document)?;
    context.guard(Some(running))?;
    require_tree(runtime, services, false)?;
    if let Some(proof) = stopped {
        stopped_tree(runtime, context, services, proof)?;
    }
    runtime.call(
        "cache-closed-port",
        json!({"request":services["requests"]["redis"],"runtime":services["runtime"]}),
    )?;
    check(runtime.now() < deadline, "关闭恢复发布后原期限或状态变化")?;
    check(
        history::evidence::recovery_evidence(
            &context.run,
            failed,
            &history["active_generation"],
            &context.sources(),
            services,
            &binding(&recovery_file)?,
        )? == proof.1["evidence"],
        "关闭恢复发布后证据变化",
    )?;
    check(
        runtime.now() < deadline,
        "关闭恢复证据核验后已超过本次原期限",
    )?;
    if let Some(proof) = stopped {
        stopped_tree(runtime, context, services, proof)?;
        check(
            runtime.now() < deadline,
            "关闭恢复完整证明复核超过本次原期限",
        )?;
    }
    result(
        context,
        "reconcile",
        "failed_close_reconciled",
        json!({"controller":controller,"owner":context.value["sources"]["state_before"],
        "failed_attempt":failed["number"],"services":document["services"],"reconciliation":binding(&file)?}),
    )
}

fn candidate(
    context: &Context,
    history: &Value,
    failed: &Value,
    services: &Value,
) -> Result<Value> {
    history::evidence::close_candidate(
        &context.run,
        failed,
        &history["active_generation"],
        &context.sources(),
        services,
        false,
    )
}

fn stable(
    runtime: &mut dyn Runtime,
    context: &Context,
    history: &Value,
    failed: &Value,
    services: &Value,
    original: &Value,
    stage: (&Value, bool, Option<&TreeProof>),
) -> Result<()> {
    let (running, tree_running, stopped) = stage;
    context.guard(Some(running))?;
    context.no_unknown()?;
    check(
        history::evidence::close_candidate(
            &context.run,
            failed,
            &history["active_generation"],
            &context.sources(),
            services,
            !tree_running,
        )? == *original,
        "关闭恢复原失败前后像、退出时间或请求变化",
    )?;
    require_tree(runtime, services, tree_running)?;
    if let Some(proof) = stopped {
        stopped_tree(runtime, context, services, proof)?;
    }
    Ok(())
}

pub(super) fn qualify(
    runtime: &mut dyn Runtime,
    context: &Context,
    history: &Value,
    failed: &Value,
    services: &Value,
) -> Result<Option<TreeProof>> {
    // 首轮进程观察前冻结有无事实及文件身份，不能接纳观察期间补出的关闭证明。
    let proof = TreeProof::capture(services)?;
    candidate(context, history, failed, services)?;
    cache_observation(runtime, services)?;
    let mut alive = 0;
    for role in ["supervisor", "monitor", "process"] {
        let expected = &services["tree"][role];
        let actual = runtime.call("identity", json!({"pid":expected["pid"]}))?;
        check(
            actual.is_null() || actual == *expected,
            "关闭恢复原RustFS成员PID复用或身份未知",
        )?;
        alive += usize::from(!actual.is_null());
    }
    check(alive == 0 || alive == 3, "关闭恢复原RustFS树仅部分退出")?;
    if alive == 3 {
        check(
            proof.files.len() == 1,
            "存活RustFS树含未确认关闭请求或完成证明",
        )?;
        require_tree(runtime, services, true)?;
        check(
            observe(runtime, services)?
                == json!({"redis":"stopped","rustfs":"running","termination":null}),
            "关闭恢复资格要求原Redis停止和原RustFS完整身份仍在",
        )?;
        proof.verify(services)?;
        Ok(None)
    } else {
        stopped_tree(runtime, context, services, &proof)?;
        Ok(Some(proof))
    }
}

fn stopped_tree(
    runtime: &mut dyn Runtime,
    context: &Context,
    services: &Value,
    proof: &TreeProof,
) -> Result<()> {
    proof.verify(services)?;
    let expected = proof
        .files
        .iter()
        .find(|file| file.path == proof.members)
        .ok_or("关闭恢复缺少首轮观察前已有的完整成员证明")?;
    require_tree(runtime, services, false)?;
    let completion = runtime.call("completion", json!({"tree":services["tree"]}))?;
    check(
        completion["bytes"] == expected.binding["bytes"]
            && completion["sha256"] == expected.binding["sha256"]
            && same_file::Handle::from_path(Path::new(text(&completion, "path")?))?
                == expected.identity,
        "关闭恢复完整成员证明与首轮冻结不符",
    )?;
    history::evidence::members(
        &json!({"state":"stopped","tree":services["tree"],"completion":completion}),
        &context.run,
    )?;
    check(
        runtime.call(
            "storage-closed",
            json!({"request":services["requests"]["rustfs"]}),
        )? == true,
        "关闭恢复原RustFS端口未明确关闭",
    )?;
    require_tree(runtime, services, false)?;
    proof.verify(services)
}

fn require_tree(runtime: &mut dyn Runtime, services: &Value, running: bool) -> Result<()> {
    for role in ["supervisor", "monitor", "process"] {
        let expected = &services["tree"][role];
        let actual = runtime.call("identity", json!({"pid":expected["pid"]}))?;
        check(
            if running {
                actual == *expected
            } else {
                actual.is_null()
            },
            "关闭恢复原RustFS成员实际身份不符、PID复用或未退出",
        )?;
    }
    if running {
        let root = Path::new(text(&services["tree"], "runtime_directory")?);
        let operation = text(&services["tree"], "operation_id")?;
        check(
            !root
                .join(format!("rustfs-members-{operation}-stopped.json"))
                .try_exists()?,
            "存活RustFS树已有完整关闭证明，拒绝推断当前代次",
        )?;
        for suffix in ["control", "result"] {
            check(
                !root
                    .join(format!("rustfs-tree-{operation}-{suffix}.json"))
                    .try_exists()?,
                "原RustFS树存在未确认关闭请求或结果",
            )?;
        }
        check(runtime.call("storage-running",json!({"identity":services["tree"]["process"],"request":services["requests"]["rustfs"]}))? == true,"关闭恢复原RustFS命令、来源或监听归属变化")?;
    }
    Ok(())
}
fn cache_observation(runtime: &mut dyn Runtime, services: &Value) -> Result<Value> {
    let value = runtime.call("cache-status",json!({"request":services["requests"]["redis"],"runtime":services["runtime"],"effective":services["cache_effective"]}))?;
    if services["requests"]["redis"]["format_version"] == 3 {
        check(
            value
                == json!({"state":"stopped","launcher_alive":false,
                "windows":{"identity":services["runtime"]["process"],"alive":false}}),
            "关闭恢复未观察到原 Windows Redis 身份及监督器退出",
        )?;
        return Ok(value);
    }
    check(
        value["state"] == "stopped"
            && value["launcher_alive"] == false
            && value["linux"]["alive"] == false
            && value["linux"]["terminated"] == false
            && value["linux"]["identity"] == services["runtime"]["linux_identity"]
            && value["linux"]["boot_id"] == services["runtime"]["boot_id"],
        "关闭恢复未观察到原Linux身份及launcher退出",
    )?;
    Ok(value)
}

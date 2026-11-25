//! 复核已完成成员回收的 RustFS 和同一启动期内失联的 Windows Redis。
use super::*;
use crate::recovery::{
    clone::native::{cache::host_control, lifecycle::manifest, resources::common},
    monitoring::execution::ProtectedInputs,
    source::evidence::{host_boot_proof, verify_host_boot_proof},
};
use std::collections::BTreeSet;

const BOOT_DRIFT_TICKS: u64 = 60 * 10_000_000;

pub(super) fn observe(
    runtime: &mut dyn Runtime,
    context: &Context,
    services: &Value,
    previous: Option<&Value>,
) -> Result<Option<Value>> {
    if services["tree"]["format_version"] != 2
        || services["runtime"]["format_version"] != 3
        || services["requests"]["redis"]["format_version"] != 3
        || services["runtime"]["redis"]["platform"] != "windows"
        || !services["cache_effective"].is_null()
    {
        return Ok(None);
    }
    let rustfs = Path::new(text(&services["tree"], "runtime_directory")?);
    let redis = Path::new(text(&services["runtime"], "output")?);
    let members = observation::receipt(&services["tree"], "stopped")?;
    let failure = redis.join("failed.json");
    if !members.try_exists()? || !failure.try_exists()? {
        return Ok(None);
    }
    let member_status = read_status(&context.backend, &members, 4 * 1024 * 1024)?;
    if member_status["status"] != "stopped"
        || !member_status["error_type"].is_null()
        || read_status(&context.backend, &failure, 64 * 1024)?["cleanup_completed"] != false
    {
        return Ok(None);
    }

    let now = host_boot_proof()?;
    let recorded = previous
        .map(|value| &value["termination"]["evidence"])
        .filter(|evidence| evidence["kind"] == MIXED_KIND);
    let boot = recorded.map_or(&now, |evidence| &evidence["host_boot"]);
    let paths = Origins::new(&context.backend, &context.run, rustfs, redis)?;
    let protected = ProtectedInputs::open([
        paths.rustfs.join("request.json"),
        paths.rustfs.join("rustfs-tree.json"),
        members,
        paths.redis.join("request.json"),
        paths.redis.join("runtime.json"),
        failure,
    ])?;
    protected.finish(observe_originals(runtime, services, &paths, boot, &now))
}

fn observe_originals(
    runtime: &mut dyn Runtime,
    services: &Value,
    paths: &Origins<'_>,
    boot: &Value,
    now: &Value,
) -> Result<Option<Value>> {
    let before = paths.capture(boot)?;
    check(
        before.requests == services["requests"]
            && before.tree == services["tree"]
            && before.cache == services["runtime"]
            && same_file_binding(
                &services["evidence"]["rustfs_tree"],
                &before.evidence["rustfs_tree"],
            )?
            && same_file_binding(
                &services["evidence"]["redis_runtime"],
                &before.evidence["redis_incident"]["runtime"],
            )?,
        "混合终态与当前登记的服务请求、进程树或运行收据不同",
    )?;
    check_same_boot(boot, now)?;
    check_rustfs_absent(runtime, &before)?;
    let redis_incident = runtime.call(
        "cache-accepted-failure",
        json!({"request":before.requests["redis"],"runtime":before.cache,"live":true}),
    )?;
    check_rustfs_absent(runtime, &before)?;
    let after = paths.capture(boot)?;
    check_same_boot(boot, &host_boot_proof()?)?;
    check(
        after == before && redis_incident == before.evidence["redis_incident"],
        "混合终态核对期间原目录绑定、成员或 Redis 事故证据变化",
    )?;
    Ok(Some(
        json!({"redis":"stopped","rustfs":"external-termination-unreconciled",
        "termination":termination(before.evidence)}),
    ))
}

fn read_status(backend: &Path, filename: &Path, limit: u64) -> Result<Value> {
    let descriptor = common::binding(filename)?;
    check(
        descriptor["bytes"]
            .as_u64()
            .is_some_and(|bytes| bytes <= limit),
        "混合终态收据超过既定读取上限",
    )?;
    common::read_bound(backend, &descriptor, "混合终态原收据")
}

pub(super) fn verify_historical(
    run: &Path,
    rustfs: &Path,
    redis: &Path,
    registered: &Value,
    observation: &Value,
) -> Result<()> {
    let backend = crate::workspace::root_dir();
    let paths = Origins::new(&backend, run, rustfs, redis)?;
    let evidence = &observation["evidence"];
    let original = paths.capture(&evidence["host_boot"])?;
    verify_snapshot(registered, observation, original)
}

fn verify_snapshot(registered: &Value, observation: &Value, original: Snapshot) -> Result<()> {
    let requests = &registered["requests"];
    json_io::exact(requests, &["rustfs", "redis"], "混合终态原请求集合")?;
    check(
        same_file_binding(&registered["tree"], &original.evidence["rustfs_tree"])?
            && registered["runtime"] == original.cache
            && same_file_binding(&requests["rustfs"], &original.evidence["rustfs_request"])?
            && same_file_binding(&requests["redis"], &original.evidence["redis_request"])?,
        "历史混合终态未绑定账本的原 RustFS、Redis 请求及运行代次",
    )?;
    check(
        *observation == termination(original.evidence),
        "历史混合终态与原件或服务身份不同",
    )
}

fn same_file_binding(recorded: &Value, observed: &Value) -> Result<bool> {
    manifest::validate_binding_shape(recorded, "混合终态登记原件绑定")?;
    manifest::validate_binding_shape(observed, "混合终态实际原件绑定")?;
    Ok(
        Path::new(text(recorded, "path")?) == Path::new(text(observed, "path")?)
            && recorded["bytes"] == observed["bytes"]
            && recorded["sha256"] == observed["sha256"],
    )
}

struct Origins<'a> {
    backend: &'a Path,
    rustfs: PathBuf,
    redis: PathBuf,
}

impl<'a> Origins<'a> {
    fn new(backend: &'a Path, run: &Path, rustfs: &Path, redis: &Path) -> Result<Self> {
        let run = common::local_directory(backend, run, "混合终态服务账本")?;
        let rustfs = common::local_directory(backend, rustfs, "混合终态原 RustFS 目录")?;
        let redis = common::local_directory(backend, redis, "混合终态原 Redis 目录")?;
        for (directory, role) in [(&rustfs, "rustfs"), (&redis, "redis")] {
            check(
                directory.file_name().and_then(|value| value.to_str()) == Some(role)
                    && directory.starts_with(&run),
                "混合终态服务目录越过原账本或角色不一致",
            )?;
        }
        Ok(Self {
            backend,
            rustfs,
            redis,
        })
    }

    fn capture(&self, boot: &Value) -> Result<Snapshot> {
        let rustfs_request = common::binding(&self.rustfs.join("request.json"))?;
        let redis_request = common::binding(&self.redis.join("request.json"))?;
        let runtime = common::binding(&self.redis.join("runtime.json"))?;
        let storage = common::read_bound(self.backend, &rustfs_request, "混合终态原 RustFS 请求")?;
        let request = common::read_bound(self.backend, &redis_request, "混合终态原 Redis 请求")?;
        let cache = common::read_bound(self.backend, &runtime, "混合终态原 Redis 运行收据")?;
        let (tree, rustfs_tree, rustfs_members) = readonly::read_rustfs_completion(
            self.backend,
            &self.rustfs,
            text(&storage, "scope_id")?,
        )?;
        check(
            cache["format_version"] == 3
                && request["format_version"] == 3
                && cache["redis"]["platform"] == "windows"
                && Path::new(text(&cache, "output")?) == self.redis,
            "混合终态 Redis 请求或运行收据不属于原 Windows 代次",
        )?;
        check_started(boot, &tree, &cache)?;
        let incident = host_control::accepted_failure(self.backend, &request, &cache, false)?;
        check(
            incident["kind"] == history::evidence::INCOMPLETE_REDIS_CLEANUP
                && same_file_binding(&incident["runtime"], &runtime)?
                && incident["normal_job_exit_proof"].is_null(),
            "混合终态缺少原 Redis incomplete 事故且禁止生成正常退出证明",
        )?;
        Ok(Snapshot {
            requests: json!({"rustfs":storage,"redis":request}),
            tree,
            cache,
            evidence: json!({"kind":MIXED_KIND,"host_boot":boot,"rustfs_request":rustfs_request,
                "rustfs_tree":rustfs_tree,"rustfs_members":rustfs_members,
                "redis_request":redis_request,"redis_incident":incident}),
        })
    }
}

#[derive(PartialEq)]
struct Snapshot {
    requests: Value,
    tree: Value,
    cache: Value,
    evidence: Value,
}

fn check_same_boot(recorded: &Value, current: &Value) -> Result<()> {
    let recorded = verify_host_boot_proof(recorded)?;
    let current = verify_host_boot_proof(current)?;
    check(
        recorded.abs_diff(current) <= BOOT_DRIFT_TICKS,
        "当前主机启动刻度与混合终态原件不一致",
    )
}

fn check_started(boot: &Value, tree: &Value, cache: &Value) -> Result<()> {
    let lower = verify_host_boot_proof(boot)?
        .checked_add(BOOT_DRIFT_TICKS)
        .ok_or("混合终态主机启动刻度溢出")?;
    let upper = boot["observed_ticks"]
        .as_u64()
        .ok_or("混合终态缺少采集刻度")?;
    check_identity_window(lower, upper, tree, cache)
}

fn check_identity_window(lower: u64, upper: u64, tree: &Value, cache: &Value) -> Result<()> {
    let mut pids = BTreeSet::new();
    for identity in [
        &tree["supervisor"],
        &tree["monitor"],
        &tree["process"],
        &cache["supervisor"],
        &cache["process"],
    ] {
        common::validate_process_identity(identity)?;
        let pid = identity["pid"].as_u64().ok_or("混合终态身份缺少 PID")?;
        let started = text(identity, "started")?.parse::<u64>()?;
        check(
            u32::try_from(pid).is_ok() && pids.insert(pid) && started > lower && started <= upper,
            "混合终态身份的 PID 或创建刻度与同一主机启动期不一致",
        )?;
    }
    Ok(())
}

fn check_rustfs_absent(runtime: &mut dyn Runtime, original: &Snapshot) -> Result<()> {
    for role in ["supervisor", "monitor", "process"] {
        let identity = &original.tree[role];
        let actual = runtime.call("identity", json!({"pid":identity["pid"]}))?;
        require_absent(identity, &actual)?;
    }
    check(
        runtime.call(
            "storage-closed",
            json!({"request":original.requests["rustfs"]}),
        )? == true,
        "混合终态原 RustFS 端口释放未明确核验",
    )
}

fn require_absent(expected: &Value, actual: &Value) -> Result<()> {
    if actual.is_null() {
        return Ok(());
    }
    check(expected != actual, "混合终态原 RustFS 身份仍可见")?;
    Err("混合终态 RustFS 原 PID 已复用，保留现场".into())
}

fn termination(evidence: Value) -> Value {
    json!({"status":"external-termination-unreconciled",
        "identities":{"supervisor":"missing","monitor":"missing","process":"missing"},
        "normal_shutdown_proof":null,"evidence":evidence})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(pid: u32, started: u64) -> Value {
        json!({"pid":pid,"started":started.to_string(),"executable":r"D:\fixture\supervisor.exe"})
    }

    #[test]
    fn mixed_identities_require_distinct_pids_and_the_same_boot_window() {
        let tree = json!({"supervisor":identity(11,101),"monitor":identity(12,102),"process":identity(13,103)});
        let cache = json!({"supervisor":identity(14,104),"process":identity(15,105)});
        assert!(check_identity_window(100, 200, &tree, &cache).is_ok());
        for invalid in [identity(13, 105), identity(15, 100), identity(15, 201)] {
            let mut changed = cache.clone();
            changed["process"] = invalid;
            assert!(check_identity_window(100, 200, &tree, &changed).is_err());
        }
        assert!(require_absent(&tree["process"], &Value::Null).is_ok());
        assert!(require_absent(&tree["process"], &tree["process"]).is_err());
        assert!(require_absent(&tree["process"], &identity(13, 107)).is_err());
    }

    #[test]
    fn mixed_history_binds_the_two_sources_and_retains_both_incident_facts() {
        let descriptor = |path: &str| json!({"path":path,"bytes":298,"sha256":"a".repeat(64)});
        let registered = json!({"tree":descriptor(r"D:\fixture\lifecycle-0007\rustfs/rustfs-tree.json"),
            "runtime":{"output":"lifecycle-0008/redis"},
            "requests":{"rustfs":descriptor(r"D:\fixture\lifecycle-0007\rustfs/request.json"),
                "redis":descriptor(r"D:\fixture\lifecycle-0008\redis/request.json")}});
        let evidence = json!({"kind":MIXED_KIND,
            "rustfs_tree":descriptor(r"D:\fixture\lifecycle-0007\rustfs\rustfs-tree.json"),
            "rustfs_members":{"path":"lifecycle-0007/rustfs/members.json"},
            "rustfs_request":descriptor(r"D:\fixture\lifecycle-0007\rustfs\request.json"),
            "redis_request":descriptor(r"D:\fixture\lifecycle-0008\redis\request.json"),
            "redis_incident":{"kind":history::evidence::INCOMPLETE_REDIS_CLEANUP,
                "normal_job_exit_proof":null}});
        let snapshot = || Snapshot {
            requests: Value::Null,
            tree: Value::Null,
            cache: registered["runtime"].clone(),
            evidence: evidence.clone(),
        };
        let observation = termination(evidence.clone());
        assert!(verify_snapshot(&registered, &observation, snapshot()).is_ok());
        for (field, value) in [
            ("rustfs_members", Value::Null),
            ("redis_incident", json!({"normal_job_exit_proof":true})),
        ] {
            let mut changed = observation.clone();
            changed["evidence"][field] = value;
            assert!(verify_snapshot(&registered, &changed, snapshot()).is_err());
        }
        for (field, value) in [
            (
                "path",
                json!(r"D:\fixture\lifecycle-0008\rustfs/request.json"),
            ),
            ("sha256", json!("b".repeat(64))),
            ("bytes", json!(299)),
            ("unexpected", json!(true)),
        ] {
            let mut wrong = registered.clone();
            wrong["requests"]["rustfs"][field] = value;
            assert!(verify_snapshot(&wrong, &observation, snapshot()).is_err());
        }
        let mut rewritten = observation;
        rewritten["evidence"]["redis_request"]["path"] =
            registered["requests"]["redis"]["path"].clone();
        assert!(verify_snapshot(&registered, &rewritten, snapshot()).is_err());
    }
}

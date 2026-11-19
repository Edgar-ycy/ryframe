//! 核对 Windows 重启前的 RustFS 监督事故；历史失败仍是失败。
use super::{closed, receipt, *};
use crate::recovery::{
    clone::native::digest, monitoring::execution::ProtectedInputs,
    source::evidence::reboot_process_exited,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
};

const START_FILES: &[&str] = &[
    "actor-binding.json",
    "actor.json",
    "intent.json",
    "launch.json",
    "process.json",
    "ready.json",
    "request.json",
    "rustfs.json",
    "spawned.json",
    "stderr.log",
    "stdout.log",
];
const EVIDENCE_KEYS: [&str; 6] = [
    "tree",
    "ready",
    "product",
    "member_failure",
    "actor_failure",
    "diagnostic",
];

pub(in super::super) fn rustfs_reboot_incident(
    runtime: &mut dyn Runtime,
    backend: &Path,
    services: &Value,
    boot: u64,
) -> Result<Value> {
    capture(backend, services, boot, |identities| {
        let request = &services["requests"]["rustfs"];
        for _ in 0..2 {
            absent(runtime, identities, boot)?;
            closed(request)?;
        }
        Ok(())
    })
}

pub(in super::super) fn rustfs_reboot_evidence(
    backend: &Path,
    services: &Value,
    boot: u64,
) -> Result<Value> {
    capture(backend, services, boot, |_| Ok(()))
}

type Originals = BTreeMap<u32, BTreeSet<String>>;

fn capture(
    backend: &Path,
    services: &Value,
    boot: u64,
    observe: impl FnOnce(&Originals) -> Result<()>,
) -> Result<Value> {
    let tree = &services["tree"];
    let root = Path::new(text(tree, "runtime_directory")?);
    let paths = [
        root.join("rustfs-tree.json"),
        receipt(tree, "ready")?,
        receipt(tree, "product")?,
        receipt(tree, "stopped")?,
        root.join("actor-failed.json"),
        root.join("actor-failure-diagnostic.json"),
    ];
    let initial = directory(backend, root, &paths)?;
    let protected = ProtectedInputs::open(paths.iter().cloned())?;
    protected.finish((|| {
        let mut evidence = json!({"normal_shutdown_proof": null});
        let mut values = Vec::new();
        for (index, (key, path)) in EVIDENCE_KEYS.into_iter().zip(&paths).enumerate() {
            let descriptor = common::binding(path)?;
            let limit = if index == 3 {
                4 * 1024 * 1024
            } else {
                64 * 1024
            };
            check(
                descriptor["bytes"]
                    .as_u64()
                    .is_some_and(|size| size <= limit),
                "RustFS 事故证据超过读取上限",
            )?;
            values.push(common::read_bound(backend, &descriptor, "RustFS 事故证据")?);
            evidence[key] = descriptor;
        }
        check(
            evidence["tree"] == services["evidence"]["rustfs_tree"] && values[0] == *tree,
            "RustFS 事故进程树不是原登记",
        )?;
        let originals = records(tree, &services["requests"]["rustfs"], &values, boot)?;
        observe(&originals)?;
        check(
            directory(backend, root, &paths)? == initial,
            "RustFS 事故目录在复核期间变化",
        )?;
        Ok(evidence)
    })())
}

fn directory(backend: &Path, root: &Path, evidence: &[PathBuf]) -> Result<(u64, u64)> {
    common::local_directory(backend, root, "RustFS 事故原运行目录")?;
    let mut expected: BTreeSet<_> = START_FILES
        .iter()
        .copied()
        .map(std::ffi::OsString::from)
        .collect();
    for path in evidence {
        let name = path.file_name().ok_or("RustFS 事故证据缺少文件名")?;
        check(expected.insert(name.to_owned()), "RustFS 事故证据文件重复")?;
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        check(
            metadata.is_file() && !digest::linked(&metadata) && expected.remove(&entry.file_name()),
            "RustFS 事故目录含未登记、链接或与事故矛盾的关闭证据",
        )?;
    }
    check(expected.is_empty(), "RustFS 事故目录缺少原登记文件")?;
    common::device_inode_for(root, &fs::symlink_metadata(root)?)
}

fn records(tree: &Value, request: &Value, values: &[Value], boot: u64) -> Result<Originals> {
    json_io::exact(
        tree,
        &[
            "format_version",
            "kind",
            "runtime_directory",
            "role",
            "scope_id",
            "operation_id",
            "supervisor",
            "process",
            "monitor",
            "group_id",
        ],
        "RustFS 事故原进程树",
    )?;
    check(
        tree["format_version"] == 2
            && tree["kind"] == "full-stack-process-tree"
            && tree["role"] == "rustfs"
            && tree["scope_id"] == request["scope_id"]
            && tree["group_id"] == tree["supervisor"]["pid"]
            && ["supervisor", "process", "monitor"]
                .map(|role| tree[role]["pid"].as_u64())
                .into_iter()
                .collect::<BTreeSet<_>>()
                .len()
                == 3,
        "RustFS 事故原进程树与登记角色、进程组或 scope 不符",
    )?;
    let [_, ready, product, members, actor, diagnostic] = values else {
        return Err(model::error("RustFS 事故原证据文件集合不完整"));
    };
    let mut owner = json!({"format_version":1,"operation_id":tree["operation_id"],
        "directory":tree["runtime_directory"],"role":tree["role"],"scope_id":tree["scope_id"],
        "supervisor":tree["supervisor"],"monitor":tree["monitor"]});
    check(
        *ready == owner
            && *product
                == json!({"operation_id":tree["operation_id"],
                "identity":tree["process"],"monitor":tree["monitor"]}),
        "RustFS 事故的原监督与产品句柄登记不符",
    )?;
    owner["status"] = json!("failed");
    owner["error_type"] = json!("MemberCleanupFailed");
    owner["members"] = members["members"].clone();
    check(
        *members == owner
            && *actor
                == json!({"operation_id":tree["operation_id"],"stage":"supervision",
                "outcome":"unknown","error_type":"SupervisionFailed"})
            && *diagnostic
                == json!({"format_version":1,"operation_id":tree["operation_id"],
                "observation":{"category":"io","os_code":1450},"cleanup":{"category":"unknown"}}),
        "RustFS 事故失败或诊断证据不属于已登记的 Windows 资源不足现场",
    )?;
    member_identities(
        tree,
        json_io::array(&members["members"], "RustFS 已观察成员")?,
        boot,
    )
}

fn member_identities(tree: &Value, members: &[Value], boot: u64) -> Result<Originals> {
    check(
        members.contains(&tree["supervisor"])
            && members.contains(&tree["process"])
            && !members.contains(&tree["monitor"]),
        "RustFS 事故成员不含原监督与产品，或混入 Job 外 monitor",
    )?;
    let mut identities = Originals::new();
    for identity in members.iter().chain(std::iter::once(&tree["monitor"])) {
        check(
            reboot_process_exited(identity, boot)?,
            "RustFS 事故已观察身份没有早于主机重启余量",
        )?;
        let pid = u32::try_from(identity["pid"].as_u64().ok_or("RustFS 进程 PID 无效")?)?;
        let started = text(identity, "started")?.to_owned();
        check(
            identities.entry(pid).or_default().insert(started),
            "RustFS 事故成员含重复 PID 与创建时间",
        )?;
    }
    Ok(identities)
}

fn absent(runtime: &mut dyn Runtime, originals: &Originals, boot: u64) -> Result<()> {
    for (pid, starts) in originals {
        let current = runtime.call("identity", json!({"pid":pid}))?;
        if !current.is_null() {
            common::validate_process_identity(&current)?;
            check(
                current["pid"] == *pid
                    && !starts.contains(text(&current, "started")?)
                    && !reboot_process_exited(&current, boot)?,
                "RustFS 事故原身份仍可见或当前进程与重启时序矛盾",
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    const BOOT: u64 = 134_400_000_000_000_000;
    const PRIOR: u64 = BOOT - 300 * 10_000_000 - 1;
    const OPERATION: &str = "2b520f68bc9d4d14889b46178383d557";

    struct Fixture {
        backend: PathBuf,
        root: tempfile::TempDir,
        services: Value,
    }

    impl Fixture {
        fn new() -> Self {
            let backend = crate::workspace::root_dir();
            let base = backend.join(".local-tests/rustfs-incident-evidence");
            fs::create_dir_all(&base).unwrap();
            let root = tempfile::tempdir_in(base).unwrap();
            let tree = json!({
                "format_version":2,"kind":"full-stack-process-tree","runtime_directory":root.path(),
                "role":"rustfs","scope_id":"incident-rustfs","operation_id":OPERATION,
                "supervisor":identity(1001, PRIOR),"process":identity(1002, PRIOR),
                "monitor":identity(1003, PRIOR),"group_id":1001
            });
            for name in START_FILES {
                fs::write(root.path().join(name), b"").unwrap();
            }
            let tree_path = root.path().join("rustfs-tree.json");
            json_io::write_new(&tree_path, &tree).unwrap();
            let mut ready = json!({"format_version":1,"operation_id":OPERATION,
                "directory":root.path(),"role":"rustfs","scope_id":"incident-rustfs",
                "supervisor":tree["supervisor"],"monitor":tree["monitor"]});
            json_io::write_new(&receipt(&tree, "ready").unwrap(), &ready).unwrap();
            json_io::write_new(
                &receipt(&tree, "product").unwrap(),
                &json!({
                    "operation_id":OPERATION,"identity":tree["process"],"monitor":tree["monitor"]
                }),
            )
            .unwrap();
            ready["status"] = json!("failed");
            ready["error_type"] = json!("MemberCleanupFailed");
            ready["members"] = json!([tree["supervisor"], tree["process"], identity(1004, PRIOR)]);
            json_io::write_new(&receipt(&tree, "stopped").unwrap(), &ready).unwrap();
            json_io::write_new(&root.path().join("actor-failed.json"), &json!({
                "operation_id":OPERATION,"stage":"supervision","outcome":"unknown","error_type":"SupervisionFailed"
            })).unwrap();
            json_io::write_new(
                &root.path().join("actor-failure-diagnostic.json"),
                &json!({
                    "format_version":1,"operation_id":OPERATION,
                    "observation":{"category":"io","os_code":1450},"cleanup":{"category":"unknown"}
                }),
            )
            .unwrap();
            let services = json!({"tree":tree,"requests":{"rustfs":{"scope_id":"incident-rustfs"}},
                "evidence":{"rustfs_tree":common::binding(&tree_path).unwrap()}});
            Self {
                backend,
                root,
                services,
            }
        }

        fn history(&self) -> Result<Value> {
            rustfs_reboot_evidence(&self.backend, &self.services, BOOT)
        }

        fn member_path(&self, name: &str) -> PathBuf {
            receipt(&self.services["tree"], name).unwrap()
        }

        fn edit(path: &Path, change: impl FnOnce(&mut Value)) {
            let mut value = json_io::read(path).unwrap();
            change(&mut value);
            fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
        }
    }

    fn identity(pid: u32, start: u64) -> Value {
        json!({"pid":pid,"started":start.to_string(),"executable":std::env::current_exe().unwrap()})
    }

    #[test]
    fn binds_original_failed_member_receipt_without_manufacturing_normal_proof() {
        let fixture = Fixture::new();
        let observed = fixture.history().unwrap();
        assert_eq!(observed["normal_shutdown_proof"], Value::Null);
        assert_eq!(
            observed["member_failure"],
            common::binding(&fixture.member_path("stopped")).unwrap()
        );
        assert_eq!(observed, fixture.history().unwrap());
        assert_eq!(observed.as_object().unwrap().len(), 7);
        fs::write(
            fixture
                .root
                .path()
                .join(format!("rustfs-tree-{OPERATION}-control.json")),
            b"{}",
        )
        .unwrap();
        assert!(fixture.history().unwrap_err().to_string().contains("矛盾"));
    }

    #[test]
    fn rejects_foreign_product_changed_failure_and_late_or_repeated_members() {
        let fixture = Fixture::new();
        let path = fixture.member_path("product");
        let original = fs::read(&path).unwrap();
        Fixture::edit(&path, |value| value["operation_id"] = json!("x".repeat(32)));
        assert!(
            fixture
                .history()
                .unwrap_err()
                .to_string()
                .contains("句柄登记")
        );
        fs::write(&path, original).unwrap();
        let path = fixture.root.path().join("actor-failure-diagnostic.json");
        let original = fs::read(&path).unwrap();
        Fixture::edit(&path, |value| value["observation"]["os_code"] = json!(5));
        assert!(
            fixture
                .history()
                .unwrap_err()
                .to_string()
                .contains("资源不足")
        );
        fs::write(&path, original).unwrap();
        let path = fixture.member_path("stopped");
        Fixture::edit(&path, |value| {
            value["members"][2] = identity(1004, PRIOR + 1)
        });
        assert!(
            fixture
                .history()
                .unwrap_err()
                .to_string()
                .contains("重启余量")
        );
        Fixture::edit(&path, |value| {
            value["members"][2] = value["members"][0].clone()
        });
        assert!(
            fixture
                .history()
                .unwrap_err()
                .to_string()
                .contains("重复 PID")
        );
    }

    #[derive(Default)]
    struct ProcessObservation {
        seen: Vec<u32>,
        live: bool,
    }

    impl Runtime for ProcessObservation {
        fn call(&mut self, operation: &str, args: Value) -> Result<Value> {
            assert_eq!(operation, "identity");
            let pid = u32::try_from(args["pid"].as_u64().unwrap()).unwrap();
            self.seen.push(pid);
            Ok(if pid == 1004 {
                identity(pid, if self.live { PRIOR } else { BOOT + 1 })
            } else {
                Value::Null
            })
        }

        fn now(&self) -> Duration {
            Duration::ZERO
        }

        fn pause(&mut self, _: Duration) {
            unreachable!();
        }
    }

    #[test]
    fn live_recovery_rechecks_known_members_and_an_occupied_port_only_in_live_path() {
        let mut fixture = Fixture::new();
        let api = TcpListener::bind("127.0.0.1:0").unwrap();
        let console = TcpListener::bind("127.0.0.1:0").unwrap();
        let request = &mut fixture.services["requests"]["rustfs"];
        request["api_url"] = json!(format!("http://{}", api.local_addr().unwrap()));
        request["console_url"] = json!(format!("http://{}", console.local_addr().unwrap()));
        let observe = |process: &mut ProcessObservation| {
            rustfs_reboot_incident(process, &fixture.backend, &fixture.services, BOOT)
        };
        assert!(fixture.history().is_ok());
        let mut process = ProcessObservation::default();
        assert!(
            observe(&mut process)
                .unwrap_err()
                .to_string()
                .contains("端口")
        );
        drop((api, console));
        process.seen.clear();
        assert_eq!(observe(&mut process).unwrap(), fixture.history().unwrap());
        assert_eq!(process.seen, [1001, 1002, 1003, 1004].repeat(2));
        process.live = true;
        assert!(
            observe(&mut process)
                .unwrap_err()
                .to_string()
                .contains("原身份仍可见")
        );
    }
}

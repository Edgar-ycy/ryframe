use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use serde_json::json;
use tempfile::tempdir;

use super::devex::{
    DevexCgroupOperation, DevexCgroupOptions,
    cgroup::{
        CgroupPhase, CgroupPlan, GateError, KernelIdentity, ParentState, ProcessFact,
        ProcessSnapshot, TEST_CASE, append_registry, cargo_command, create_registry,
        inspect_cleanup, parent_state_from, parse_events, privileged_identity_matches, read_json,
        read_registry, register_children, registry_path, validate_plan_at, validate_test_output,
        verify_processes, write_failure, write_json_new,
    },
    cgroup_plan_at,
};
use super::workspace::root_dir;

fn plan(backend: &Path, cgroup_root: &Path) -> CgroupPlan {
    let nonce = "a".repeat(32);
    CgroupPlan {
        format_version: 1,
        run_id: "123".to_owned(),
        attempt: "2".to_owned(),
        root: CgroupPlan::root_for(cgroup_root, "123", "2", &nonce),
        nonce,
        source_sha: "b".repeat(40),
        uid: 1001,
        gid: 1001,
        backend: backend.to_path_buf(),
        cargo: backend.join("cargo"),
        environment: BTreeMap::from([
            ("HOME".to_owned(), "fixed-home".to_owned()),
            ("PATH".to_owned(), "fixed-path".to_owned()),
        ]),
    }
}

fn fact(pid: u32, started: &str, parent: u32, uid: u32) -> ProcessFact {
    ProcessFact {
        pid,
        started: started.to_owned(),
        parent,
        uid,
    }
}

#[test]
fn cgroup_plan_requires_exact_ci_run_root_identity_and_environment() {
    let directory = tempdir().expect("创建 cgroup 计划测试目录");
    let backend = directory.path().join("backend");
    let cgroup_root = directory.path().join("cgroup");
    fs::create_dir_all(&backend).expect("创建计划 backend");
    let valid = plan(&backend, &cgroup_root);
    validate_plan_at(&valid, &backend, &cgroup_root).expect("合法计划应通过");
    assert!(privileged_identity_matches(&valid, "1001", "1001"));
    assert!(!privileged_identity_matches(&valid, "0", "1001"));
    assert!(!privileged_identity_matches(&valid, "1001", "0"));

    let cases = [
        CgroupPlan {
            run_id: "../1".to_owned(),
            ..valid.clone()
        },
        CgroupPlan {
            attempt: "0".to_owned(),
            ..valid.clone()
        },
        CgroupPlan {
            uid: 0,
            ..valid.clone()
        },
        CgroupPlan {
            root: cgroup_root.clone(),
            ..valid.clone()
        },
        CgroupPlan {
            source_sha: "unknown".to_owned(),
            ..valid.clone()
        },
        CgroupPlan {
            cargo: PathBuf::from("cargo"),
            ..valid.clone()
        },
        CgroupPlan {
            environment: BTreeMap::from([(
                "APP_DATABASE_PASSWORD".to_owned(),
                "never-persist".to_owned(),
            )]),
            ..valid.clone()
        },
    ];
    for invalid in cases {
        assert!(
            validate_plan_at(&invalid, &backend, &cgroup_root).is_err(),
            "非法计划应失败：{invalid:?}"
        );
    }

    let unknown = json!({
        "format_version": 1,
        "run_id": "123",
        "attempt": "2",
        "nonce": "a".repeat(32),
        "source_sha": "b".repeat(40),
        "uid": 1001,
        "gid": 1001,
        "root": valid.root,
        "backend": backend,
        "cargo": valid.cargo,
        "environment": {},
        "unexpected": true
    });
    assert!(serde_json::from_value::<CgroupPlan>(unknown).is_err());
}

#[test]
fn cgroup_output_must_remain_in_the_current_ignored_workspace() {
    let directory = tempdir().expect("创建 cgroup 输出测试目录");
    let backend = directory.path().join("backend");
    fs::create_dir_all(backend.join(".local-tests")).expect("创建 .local-tests");
    let inside =
        super::devex::cgroup::resolve_output_at(Path::new(".local-tests/含 空格/cgroup"), &backend)
            .expect("忽略目录内路径应通过");
    assert_eq!(inside, backend.join(".local-tests/含 空格/cgroup"));
    assert!(super::devex::cgroup::resolve_output_at(Path::new("../outside"), &backend).is_err());
    assert!(super::devex::cgroup::resolve_output_at(Path::new(".local-tests"), &backend).is_err());
    let outside = directory.path().join("outside");
    assert!(super::devex::cgroup::resolve_output_at(&outside, &backend).is_err());
}

#[test]
fn parent_cgroup_requires_cgroup2_mount_and_memory_controller_without_mutation() {
    let root = Path::new("/sys/fs/cgroup");
    let identity = KernelIdentity {
        device: 31,
        inode: 41,
    };
    let mount = "31 24 0:26 / /sys/fs/cgroup rw,nosuid - cgroup2 cgroup2 rw\n";
    let state = parent_state_from(mount, "cpu memory pids\n", root, identity.clone())
        .expect("cgroup2 与 memory 均存在时应通过");
    assert_eq!(state.identity, identity);
    assert_eq!(state.controllers, ["cpu", "memory", "pids"]);
    assert!(parent_state_from(mount, "cpu pids\n", root, identity.clone()).is_err());
    assert!(
        parent_state_from(
            "31 24 0:26 / /sys/fs/cgroup rw - tmpfs tmpfs rw\n",
            "memory\n",
            root,
            identity
        )
        .is_err()
    );
}

#[test]
fn cgroup_events_reject_missing_duplicate_or_extra_fields() {
    let events = parse_events("populated 1\nfrozen   1\n").expect("合法事件应解析");
    assert_eq!(events.get("frozen").map(String::as_str), Some("1"));
    assert!(parse_events("populated 1\npopulated 0\n").is_err());
    assert!(parse_events("frozen\n").is_err());
    assert!(parse_events("frozen 1 extra\n").is_err());
}

#[cfg(unix)]
#[test]
fn cgroup_output_rejects_symlink_components() {
    use std::os::unix::fs::symlink;

    let directory = tempdir().expect("创建符号链接测试目录");
    let backend = directory.path().join("backend");
    let local_tests = backend.join(".local-tests");
    let target = directory.path().join("target");
    fs::create_dir_all(&local_tests).expect("创建 .local-tests");
    fs::create_dir(&target).expect("创建链接目标");
    symlink(&target, local_tests.join("link")).expect("创建符号链接");
    assert!(
        super::devex::cgroup::resolve_output_at(Path::new(".local-tests/link/cgroup"), &backend)
            .is_err()
    );
}

#[test]
fn cgroup_evidence_is_create_only_and_plan_preview_writes_nothing() {
    let directory = tempdir().expect("创建 cgroup 证据测试目录");
    let path = directory.path().join("evidence.json");
    write_json_new(&path, &json!({"first": true})).expect("首次证据应创建成功");
    let error = write_json_new(&path, &json!({"second": true})).expect_err("证据不得覆盖");
    assert_eq!(error.kind(), "evidence");
    let value: serde_json::Value = read_json(&path).expect("读取已有证据");
    assert_eq!(value, json!({"first": true}));

    let output = root_dir().join(".local-tests/cgroup-plan-only");
    let options = DevexCgroupOptions {
        operation: DevexCgroupOperation::Run,
        output: output.clone(),
        plan: true,
    };
    let task_plan = cgroup_plan_at(&options, &root_dir()).expect("生成同一任务图");
    assert_eq!(task_plan.tasks.len(), 1);
    assert!(!output.exists(), "计划预览不得创建输出目录");
}

#[test]
fn phase_failure_receipt_preserves_the_original_error() {
    let directory = tempdir().expect("创建失败收据测试目录");
    let backend = directory.path().join("backend");
    let cgroup_root = directory.path().join("cgroup");
    fs::create_dir_all(&backend).expect("创建 backend");
    let plan = plan(&backend, &cgroup_root);
    let error = GateError::new("privilege", "memory denied");
    let path = write_failure(CgroupPhase::Prepare, directory.path(), Some(&plan), &error)
        .expect("写入阶段失败收据");
    let value: serde_json::Value = read_json(&path).expect("读取阶段失败收据");
    assert_eq!(value["run_id"], "123");
    assert_eq!(value["attempt"], "2");
    assert_eq!(value["error_type"], "privilege");
    assert_eq!(value["error"], "memory denied");
    assert_eq!(value["status"], "failed");
}

#[test]
fn cleanup_receipts_block_other_runs_and_late_creation_evidence() {
    let directory = tempdir().expect("创建清理收据测试目录");
    let output = directory.path().join("output");
    fs::create_dir(&output).expect("创建输出目录");
    let cgroup_root = directory.path().join("cgroup");
    let backend = directory.path().join("backend");
    fs::create_dir_all(&backend).expect("创建 backend");
    let plan = plan(&backend, &cgroup_root);

    assert_eq!(
        inspect_cleanup(&plan, &output).expect("无创建证据应可安全判定"),
        super::devex::cgroup::CleanupPreflight::NotCreated
    );
    fs::create_dir(&cgroup_root).expect("创建测试 cgroup 根");
    fs::create_dir(&plan.root).expect("创建伪造 cgroup 目录");
    assert!(inspect_cleanup(&plan, &output).is_err());
    fs::remove_dir(&plan.root).expect("撤掉伪造 cgroup 目录");
    write_json_new(
        &output.join("cleanup.json"),
        &json!({
            "run_id": "123",
            "attempt": "2",
            "status": "not_created",
            "root": plan.root,
            "terminated_registered_processes": [],
            "root_absent": true
        }),
    )
    .expect("写入未创建收据");
    assert_eq!(
        inspect_cleanup(&plan, &output).expect("未创建收据可复用"),
        super::devex::cgroup::CleanupPreflight::Reuse
    );
    let identity = KernelIdentity {
        device: 10,
        inode: 20,
    };
    let parent = ParentState {
        identity: KernelIdentity {
            device: 1,
            inode: 2,
        },
        controllers: vec!["memory".to_owned()],
    };
    write_json_new(
        &output.join("created.json"),
        &json!({
            "root": plan.root,
            "run_id": "123",
            "attempt": "2",
            "identity": identity,
            "parent": parent
        }),
    )
    .expect("写入创建证据");
    assert!(inspect_cleanup(&plan, &output).is_err());

    let changed = CgroupPlan {
        attempt: "3".to_owned(),
        root: CgroupPlan::root_for(&cgroup_root, "123", "3", &plan.nonce),
        ..plan
    };
    assert!(inspect_cleanup(&changed, &output).is_err());
}

#[test]
fn process_registration_accepts_only_known_live_ancestry() {
    let parent = fact(10, "100", 1, 1001);
    let child = fact(11, "101", 10, 1001);
    let grandchild = fact(12, "102", 11, 1001);
    let unknown = fact(13, "103", 9, 1001);
    let current = ProcessSnapshot::from([
        (10, parent.clone()),
        (11, child),
        (12, grandchild.clone()),
        (13, unknown.clone()),
    ]);
    let mut registry = ProcessSnapshot::from([(10, parent)]);
    let additions = register_children(&current, &mut registry, 1001).expect("已知 ancestry 应登记");
    assert_eq!(registry.keys().copied().collect::<Vec<_>>(), [10, 11, 12]);
    assert_eq!(additions.len(), 2);
    verify_processes(
        &ProcessSnapshot::from([(12, grandchild.clone())]),
        &registry,
        1001,
    )
    .expect("已登记孙进程应可精确清理");
    for invalid in [
        unknown,
        ProcessFact {
            started: "new".to_owned(),
            ..grandchild.clone()
        },
        ProcessFact {
            uid: 0,
            ..grandchild
        },
    ] {
        assert!(
            verify_processes(
                &ProcessSnapshot::from([(invalid.pid, invalid)]),
                &registry,
                1001
            )
            .is_err()
        );
    }
}

#[test]
fn process_registry_is_create_only_then_append_only() {
    let directory = tempdir().expect("创建进程登记测试目录");
    let path = registry_path(directory.path());
    let launcher = fact(10, "100", 1, 1001);
    let child = fact(11, "101", 10, 1001);
    create_registry(&path, &launcher).expect("首次进程登记应创建");
    assert!(
        create_registry(&path, &launcher).is_err(),
        "登记文件不得覆盖"
    );
    append_registry(&path, std::slice::from_ref(&child)).expect("后续登记应追加");
    let registry = read_registry(&path).expect("读取追加后的登记");
    assert_eq!(registry.get(&10), Some(&launcher));
    assert_eq!(registry.get(&11), Some(&child));

    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("打开进程登记");
    writeln!(
        file,
        "{}",
        serde_json::json!({
            "pid": 11,
            "started": "changed",
            "parent": 10,
            "uid": 1001
        })
    )
    .expect("追加冲突 PID");
    assert!(read_registry(&path).is_err(), "复用 PID 必须失败关闭");
}

#[test]
fn historical_or_reused_parent_pid_cannot_authorize_children() {
    let parent = fact(10, "100", 1, 1001);
    let child = fact(11, "101", 10, 1001);
    let mut registry = ProcessSnapshot::from([(10, parent.clone())]);
    register_children(
        &ProcessSnapshot::from([(11, child.clone())]),
        &mut registry,
        1001,
    )
    .expect("缺失父进程不会猜测归属");
    assert!(!registry.contains_key(&11));
    assert!(verify_processes(&ProcessSnapshot::from([(11, child)]), &registry, 1001).is_err());

    for current_parent in [
        ProcessFact {
            started: "200".to_owned(),
            ..parent.clone()
        },
        ProcessFact {
            uid: 1002,
            ..parent.clone()
        },
    ] {
        let child = fact(11, "101", 10, 1001);
        let mut registry = ProcessSnapshot::from([(10, parent.clone())]);
        assert!(
            register_children(
                &ProcessSnapshot::from([(10, current_parent), (11, child)]),
                &mut registry,
                1001
            )
            .is_err()
        );
        assert!(!registry.contains_key(&11));
    }
}

#[test]
fn cgroup_test_parser_and_cargo_command_preserve_the_exact_gate() {
    let good = format!(
        "test {TEST_CASE} ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 2 filtered out;"
    );
    validate_test_output(0, &good).expect("精确通过一次应成功");
    for (code, text) in [
        (1, good.clone()),
        (0, good.replace(TEST_CASE, "other")),
        (0, good.replace("1 passed", "0 passed")),
        (0, good.replace("0 ignored", "1 ignored")),
    ] {
        assert!(validate_test_output(code, &text).is_err());
    }
    let nested = format!(
        "test {TEST_CASE} ... \nrunning 1 test\ntest devex_memory_tests::memory_child ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 408 filtered out;\n\nok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 408 filtered out;"
    );
    validate_test_output(0, &nested).expect("嵌套子进程输出应接受最终精确结果");

    let directory = tempdir().expect("创建 Cargo 命令测试目录");
    let backend = directory.path().join("backend");
    let cgroup_root = directory.path().join("cgroup");
    let command = cargo_command(&plan(&backend, &cgroup_root));
    assert!(!command.iter().any(|argument| argument.contains(".py")));
    assert!(!command.iter().any(|argument| argument == "sudo"));
    let separator = command
        .iter()
        .position(|argument| argument == "--")
        .expect("Cargo 命令必须显式分隔测试参数");
    assert_eq!(
        &command[separator + 1..],
        [
            "--exact",
            TEST_CASE,
            "--ignored",
            "--nocapture",
            "--test-threads=1"
        ]
        .map(ToOwned::to_owned)
    );
}

#[test]
fn extended_ci_runs_only_rust_and_always_cleans_then_uploads() {
    let workflow = fs::read_to_string(root_dir().join(".github/workflows/extended-ci.yml"))
        .unwrap()
        .replace("\r\n", "\n");
    let job = workflow
        .split("\n  devex-cgroup:\n")
        .nth(1)
        .expect("找不到 devex-cgroup job")
        .split("\n  full-stack-e2e:\n")
        .next()
        .unwrap();
    assert!(!job.contains("actions/setup-python"));
    assert!(!job.contains("tools/python"));
    let run = "cargo xtask check perf cgroup run --output .local-tests/ci-devex-cgroup";
    let cleanup = "cargo xtask check perf cgroup cleanup --output .local-tests/ci-devex-cgroup";
    assert_eq!(job.matches(run).count(), 1);
    assert_eq!(job.matches(cleanup).count(), 1);
    assert!(job.contains("if: ${{ always() && steps.cgroup.outcome != 'skipped' }}"));
    assert!(job.contains("if: ${{ always() }}"));
    assert!(job.contains("path: backend/.local-tests/ci-devex-cgroup"));
    assert!(job.contains("if-no-files-found: error"));
    assert!(job.contains("ryframe-devex-cgroup-${{ github.run_id }}-${{ github.run_attempt }}"));
    assert!(job.find(run).unwrap() < job.find(cleanup).unwrap());
    assert!(job.find(cleanup).unwrap() < job.find("actions/upload-artifact@").unwrap());

    let memory = fs::read_to_string(root_dir().join("xtask/tests/internal/devex_memory.rs"))
        .expect("读取内存用例");
    assert!(memory.contains(
        "#[ignore = \"需要显式提供具有 memory controller 委托权限的 RYFRAME_DEVEX_CGROUP_ROOT\"]"
    ));
}

#[cfg(not(target_os = "linux"))]
#[test]
fn non_linux_public_run_fails_with_actionable_platform_guidance() {
    let options = DevexCgroupOptions {
        operation: DevexCgroupOperation::Run,
        output: PathBuf::from(".local-tests/cgroup-platform-rejection"),
        plan: false,
    };
    let error = super::devex::cgroup::run(&options).expect_err("Windows 必须拒绝真实运行");
    assert!(error.to_string().contains("Linux GitHub Actions"));
    assert!(error.to_string().contains("cgroup v2"));
}

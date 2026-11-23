use super::{Fixture, binding, read_json, stage, write_json};
use crate::{
    Result,
    process::{ChildGroup, ManagedChild, command_output_result_with_timeout},
    recovery::clone::native::{
        lifecycle::{identity, state},
        resources::common,
        target_verify::guard::Generation,
    },
    workspace,
};
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, Read, Write},
    net::{Ipv4Addr, TcpListener},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};
use winsafe::{HPROCESS, co, prelude::kernel_Hprocess};

const CHILD: &str = "clone_lifecycle_tests::prepare_target_lock::prepare_holder";
const READY: &str = "fixture-holder-ready";
const EXIT: i32 = 37;

#[test]
fn real_prepare_interruption_recovers_the_original_target_lock_on_the_same_run() {
    let mut fixture = Fixture::new();
    fixture.retain_evidence = true;
    eprintln!("本次合成账本与实际进程收据：{}", fixture.local.display());
    let target = setup(&fixture);
    let original = [
        fixture.run.join("manifest.json"),
        state::result_path(&fixture.run, 1),
        state::result_path(&fixture.run, 2),
        fixture.run.join("post-copy.json"),
        fixture.copy.join("result.json"),
        fixture.copy.join("ledger/head.json"),
        target.join("initialized.json"),
    ]
    .map(|path| (path.clone(), fs::read(path).unwrap()));

    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .current_dir(&fixture.backend)
        .args(["--exact", CHILD, "--ignored", "--nocapture"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null());
    let mut child = ChildGroup::new().unwrap().spawn(command).unwrap();
    let mut input = child.take_stdin_writer().unwrap().unwrap();
    writeln!(input, "{}", serde_json::to_string(&fixture.run).unwrap()).unwrap();
    input.flush().unwrap();
    wait_ready(&fixture.local.join(READY), &mut child);

    let pid = child.id();
    assert_eq!(child.job_member_pids().unwrap(), [pid]);
    let handle = HPROCESS::OpenProcess(
        co::PROCESS::QUERY_LIMITED_INFORMATION | co::PROCESS::SYNCHRONIZE,
        false,
        pid,
    )
    .unwrap();
    let process = identity::windows_snapshot(&handle, pid).unwrap();
    let controller = state::controller_path(&fixture.run, 3);
    assert_eq!(read_json(&controller)["owner"]["identity"], process);
    let original_lock = target.join("initialize.lock");
    let original_id = directory_id(&original_lock);
    assert!(fs::read_dir(&original_lock).unwrap().next().is_none());
    let observation = fixture.run.join("prepare-target-lock-0003.json");
    let observed = succeeded(cli(
        &fixture,
        "observe",
        &[
            "prepare-lock-observe",
            "--owner-binding",
            controller.to_str().unwrap(),
            "--output",
            observation.to_str().unwrap(),
        ],
    ));
    assert_eq!(observed["attempt"]["number"], 3);
    assert_eq!(observed["owner"]["identity"], process);
    assert_eq!(observed, read_json(&observation));
    let recovery = [
        "prepare-lock-recover",
        "--request",
        observation.to_str().unwrap(),
    ];
    let live = cli(&fixture, "reject-live", &recovery);
    assert!(!live.status.success());
    assert!(!observation.with_extension("recovering.json").exists());
    assert_eq!(directory_id(&original_lock), original_id);

    input.write_all(b"x").unwrap();
    drop(input);
    assert_eq!(
        handle.WaitForSingleObject(Some(5000)).unwrap(),
        co::WAIT::OBJECT_0
    );
    assert_eq!(child.try_wait().unwrap().unwrap().code(), Some(EXIT));
    assert_eq!(child.active_process_count().unwrap(), Some(0));
    write_json(
        &fixture.local.join("exit-proof.json"),
        &json!({
            "identity_before_exit": process, "same_handle_signaled": true,
            "exit_code": EXIT, "job_active_process_count": 0,
        }),
    );
    recover_interrupted(
        &fixture,
        &controller,
        &observation,
        &original_lock,
        original_id,
        &recovery,
    );

    let archive = observation.with_extension("original-empty-lock");
    let probe = stage::run_named(
        &fixture.backend,
        &fixture.run,
        "post-copy",
        "prepare",
        &Next {
            target: &target,
            original_id,
        },
    )
    .unwrap();
    assert_eq!(probe["status"], "stage_finished");
    assert_eq!(probe["attempt"], 4);
    assert_eq!(
        probe["result"],
        json!({"status":"fixture_target_lock_acquired","remote_writes":0})
    );
    assert_eq!(
        read_json(&state::result_path(&fixture.run, 4)),
        probe["result"]
    );
    let saved = fixture.state();
    assert_eq!(saved["attempts"].as_array().unwrap().len(), 4);
    assert_eq!(saved["attempts"][2]["error_type"], "ControllerInterrupted");
    assert_eq!(saved["attempts"][3]["number"], 4);
    assert_eq!(saved["attempts"][3]["status"], "passed");
    assert_eq!(directory_id(&archive), original_id);
    assert!(!original_lock.exists());
    assert!(!fixture.run.join("post-copy/runtime").exists());
    assert!(!fixture.run.join("post-copy/runtime-configuration").exists());
    for (path, bytes) in original {
        assert_eq!(fs::read(&path).unwrap(), bytes, "{}", path.display());
    }
}

fn recover_interrupted(
    fixture: &Fixture,
    controller: &Path,
    observation: &Path,
    original_lock: &Path,
    original_id: (u64, u64),
    arguments: &[&str],
) {
    let unchanged = fs::read(fixture.run.join("state.json")).unwrap();
    let premature = cli(fixture, "reject-before-outer-recovery", arguments);
    assert!(!premature.status.success());
    assert_eq!(fs::read(fixture.run.join("state.json")).unwrap(), unchanged);
    assert!(!observation.with_extension("recovering.json").exists());
    let outer = succeeded(cli(
        fixture,
        "outer-recovery",
        &["recover", "--owner-binding", controller.to_str().unwrap()],
    ));
    assert_eq!(outer["status"], "controller_recovered");
    let failed = fixture.state();
    assert_eq!(failed["attempts"][2]["stage"], "post-copy");
    assert_eq!(failed["attempts"][2]["mode"], "prepare");
    assert_eq!(failed["attempts"][2]["status"], "failed");
    assert_eq!(failed["attempts"][2]["error_type"], "ControllerInterrupted");
    assert!(failed["attempts"][2]["result"].is_null());
    assert!(!state::result_path(&fixture.run, 3).exists());
    assert_eq!(directory_id(original_lock), original_id);

    let archived = succeeded(cli(fixture, "recover-target", arguments));
    assert_eq!(archived["status"], "prepare_target_lock_recovered");
    assert_eq!(archived["remote_writes"], 0);
    assert_eq!(fixture.state(), failed);
    let archive = observation.with_extension("original-empty-lock");
    assert_eq!(directory_id(&archive), original_id);
    assert!(fs::read_dir(&archive).unwrap().next().is_none());
    assert!(!original_lock.exists());
    let receipt = observation.with_extension("recovered.json");
    let exact = fs::read(&receipt).unwrap();
    assert!(!cli(fixture, "reject-duplicate", arguments).status.success());
    assert_eq!(fs::read(receipt).unwrap(), exact);
    assert_eq!(directory_id(&archive), original_id);
}

fn setup(fixture: &Fixture) -> PathBuf {
    // 使用 Windows 默认临时端口区间以下的回环端口，避免夹具释放后被出站连接占用。
    let start = 30_000 + (std::process::id() % 8_000) as u16;
    let mut ports = (start..38_000)
        .chain(30_000..start)
        .filter_map(|port| TcpListener::bind((Ipv4Addr::LOCALHOST, port)).ok());
    let api = ports.next().expect("夹具需要独立 API 回环端口");
    let worker = ports.next().expect("夹具需要独立 Worker 回环端口");
    let target = fixture.local.join("target");
    fs::create_dir(&target).unwrap();
    let initialized = target.join("initialized.json");
    write_json(
        &initialized,
        &json!({
            "status":"fresh_target_initialized",
            "generation":{"selected":{
                "runtime_dir": fixture.local.join("synthetic-product-runtime"),
                "api_url": format!("http://127.0.0.1:{}", api.local_addr().unwrap().port()),
                "worker_ready_url": format!("http://127.0.0.1:{}/readyz", worker.local_addr().unwrap().port()),
            }},
        }),
    );
    let mut manifest = read_json(&fixture.manifest);
    manifest["initialized"] = binding(&initialized);
    write_json(&fixture.manifest, &manifest);
    fixture.init().unwrap();

    // 合成产品收据只供本地历史绑定；夹具不会执行复制、初始化或业务准备。
    fs::create_dir_all(fixture.copy.join("ledger")).unwrap();
    let copy = json!({"status":"data_steps_verified","fixture":"synthetic-local-receipt"});
    write_json(&fixture.copy.join("result.json"), &copy);
    write_json(
        &fixture.copy.join("ledger/head.json"),
        &json!({"fixture":"unchanged"}),
    );
    let (number, _) = state::begin(&fixture.run, "copy", "resume", &json!({})).unwrap();
    state::finish(&fixture.run, number, Some(&copy), None).unwrap();
    let registration = fixture.run.join("post-copy.json");
    write_json(
        &registration,
        &json!({"kind":"devex-clone-post-copy","fixture":"synthetic"}),
    );
    let (number, _) = state::begin(&fixture.run, "post-copy", "register", &json!({})).unwrap();
    state::finish(
        &fixture.run,
        number,
        Some(&json!({
            "status":"post_copy_registered","registration": binding(&registration),
        })),
        None,
    )
    .unwrap();
    target
}

fn directory_id(path: &Path) -> (u64, u64) {
    common::device_inode_for(path, &fs::symlink_metadata(path).unwrap()).unwrap()
}

fn wait_ready(path: &Path, child: &mut ManagedChild) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !path.is_file() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "合成准备子进程提前退出"
        );
        assert!(Instant::now() < deadline, "子进程未取得两把原生 guard");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn cli(fixture: &Fixture, label: &str, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
    command
        .current_dir(&fixture.backend)
        .args(["check", "recovery", "clone"])
        .args(args)
        .arg("--run-dir")
        .arg(&fixture.run)
        .arg("--write")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let out = command_output_result_with_timeout(command, Duration::from_secs(20)).unwrap();
    fs::File::create_new(fixture.local.join(format!("{label}.stdout")))
        .unwrap()
        .write_all(&out.stdout)
        .unwrap();
    fs::File::create_new(fixture.local.join(format!("{label}.stderr")))
        .unwrap()
        .write_all(&out.stderr)
        .unwrap();
    out
}

fn succeeded(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

struct Hold<'a> {
    target: &'a Path,
    ready: &'a Path,
    input: std::cell::RefCell<&'a mut dyn Read>,
}

impl stage::StageRunner for Hold<'_> {
    fn preflight(&self, _: &stage::StageRequest<'_>) -> Result<()> {
        Ok(())
    }

    fn execute(&self, request: &stage::StageRequest<'_>) -> Result<Value> {
        assert_eq!(request.attempt, Some(3));
        let owner = Generation::acquire(request.backend, self.target)?;
        owner.check()?;
        fs::create_dir_all(request.run_dir.join("post-copy/attempt-0003"))?;
        fs::File::create_new(self.ready)?.sync_all()?;
        let mut received = [0];
        self.input.borrow_mut().read_exact(&mut received)?;
        assert_eq!(received, [b'x']);
        // 模拟控制器未展开锁收尾的退出；Job 仅包含本次合成子进程。
        std::process::exit(EXIT)
    }
}

#[test]
#[ignore = "由真实准备锁生命周期测试在独占 Windows Job 中调用"]
fn prepare_holder() {
    let backend = workspace::root_dir();
    let mut input = std::io::stdin().lock();
    let mut line = String::new();
    input.read_line(&mut line).unwrap();
    let run: PathBuf = serde_json::from_str(&line).unwrap();
    let parent = run.parent().unwrap();
    assert_eq!(run, parent.join("run"));
    assert_eq!(parent.parent().unwrap(), backend.join(".local-tests"));
    assert!(
        parent
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("clone-lifecycle-")
    );
    let runner = Hold {
        target: &parent.join("target"),
        ready: &parent.join(READY),
        input: std::cell::RefCell::new(&mut input),
    };
    let result = stage::run_named(&backend, &run, "post-copy", "prepare", &runner);
    panic!("未维持合成准备子进程：{result:?}");
}

struct Next<'a> {
    target: &'a Path,
    original_id: (u64, u64),
}

impl stage::StageRunner for Next<'_> {
    fn preflight(&self, _: &stage::StageRequest<'_>) -> Result<()> {
        Ok(())
    }

    fn execute(&self, request: &stage::StageRequest<'_>) -> Result<Value> {
        assert_eq!(request.attempt, Some(4));
        let owner = Generation::acquire(request.backend, self.target)?;
        owner.check()?;
        assert_ne!(
            directory_id(&self.target.join("initialize.lock")),
            self.original_id
        );
        owner.finish(Ok(
            json!({"status":"fixture_target_lock_acquired","remote_writes":0}),
        ))
    }
}

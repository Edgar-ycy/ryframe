use super::*;
use crate::recovery::full_stack::worker::operation::Operation;

const ID: &str = "0123456789abcdef0123456789abcdef";

fn temporary() -> Result<(tempfile::TempDir, PathBuf)> {
    let root = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&root)?;
    let temporary = tempfile::tempdir_in(root)?;
    let directory = operation::canonical(temporary.path())?;
    Ok((temporary, directory))
}

#[test]
fn missing_exact_archive_never_creates_the_role_guard_or_changes_pending() -> Result<()> {
    let (_temporary, directory) = temporary()?;
    let role = Role::Worker;
    let parent = directory.join(role.archive());
    fs::create_dir_all(parent.join("a".repeat(32)))?;
    let pending = directory.join(role.lock());
    fs::create_dir(&pending)?;
    assert!(
        archived_start_bound(role, &directory, &directory, &Value::Null, &Value::Null, ID)?
            .is_none()
    );
    assert!(pending.is_dir());
    assert!(!directory.join("worker-control.guard").exists());
    assert!(
        archived_start_bound(
            role,
            &directory,
            &directory,
            &Value::Null,
            &Value::Null,
            "../../worker-control.lock"
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn archived_unknown_binds_raw_operation_and_stays_unknown_without_the_product() -> Result<()> {
    let (_temporary, directory) = temporary()?;
    let role = Role::Worker;
    let runtime = json!({"scope_id":"archived-test","worker_ready_url":"http://127.0.0.1:19131/readyz",
        "artifacts":{"worker":{"path":std::env::current_exe()?,"sha256":"a".repeat(64)}}});
    operation::write_new(&directory.join("runtime.json"), &runtime)?;
    let environment_path = directory.join("environment.json");
    operation::write_new(
        &environment_path,
        &json!({"environment":{"APP_ENV":"test"}}),
    )?;
    let environment = Environment::capture(&common::binding(&environment_path)?)?;
    let operation = Operation::claim(role, "start", &directory, &directory, &runtime, 1.0)?;
    let source = operation::source(&directory)?;
    let mut request = operation.request;
    request["environment_binding"] = environment.binding().clone();
    fs::write(
        operation.path.join("request.json"),
        serde_json::to_vec(&request)?,
    )?;
    let operation = Operation::read(role, &directory, &source)?;
    let owner = operation.owner["identity"].clone();
    let id = operation.id()?.to_owned();
    let result = operation.result("unknown", Value::Null, Some("固定启动失败"), false)?;
    operation.publish("result.json", &result, false)?;
    let raw = fs::read(operation.path.join("result.json"))?;
    operation.archive()?;
    let parent = directory.join(role.archive());
    let path = parent.join(&id);
    let archive = Archive::open(parent, path.clone())?;
    let frame = archive.frame(&directory, &directory, &source, &runtime)?;
    let mut replaced_runtime = runtime.clone();
    replaced_runtime["scope_id"] = json!("replacement");
    assert!(
        archive
            .frame(&directory, &directory, &source, &replaced_runtime)
            .is_err()
    );
    let outcome = frame.classify(role, &id, &owner, &environment)?;
    assert_eq!(outcome, "failed");
    assert_eq!(frame.result, result);
    assert!(!frame.observe(role, &directory, &runtime, &id, outcome)?);
    assert!(frame.classify(role, ID, &owner, &environment).is_err());
    assert!(
        frame
            .classify(role, &id, &Value::Null, &environment)
            .is_err()
    );
    assert!(
        frame
            .classify(Role::Api, &id, &owner, &environment)
            .is_err()
    );
    let other_path = directory.join("other");
    fs::create_dir(&other_path)?;
    let other = other_path.join("environment.json");
    operation::write_new(&other, &json!({"environment":{"APP_ENV":"test"}}))?;
    let wrong_environment = Environment::capture(&common::binding(&other)?)?;
    assert!(
        frame
            .classify(role, &id, &owner, &wrong_environment)
            .is_err()
    );
    assert_eq!(fs::read(path.join("result.json"))?, raw);
    archive.check()?;
    drop(archive);
    operation::write_new(&path.join("unrelated.json"), &json!({}))?;
    assert!(Archive::open(directory.join(role.archive()), path).is_err());
    Ok(())
}

#[test]
fn original_ready_frame_requires_the_exact_operation_and_each_member() -> Result<()> {
    let (_temporary, directory) = temporary()?;
    let role = Role::Api;
    let supervisor = json!({"pid":2,"started":"2","executable":directory.join("supervisor.exe")});
    let monitor = json!({"pid":3,"started":"3","executable":directory.join("monitor.exe")});
    let product = json!({"pid":4,"started":"4","executable":directory.join("product.exe")});
    let frame = Frame {
        owner: Value::Null,
        request: json!({"scope_id":"original"}),
        supervisor: json!({"identity":supervisor}),
        progress: json!({"identity":product}),
        result: Value::Null,
    };
    let ready = json!({"format_version":1,"operation_id":ID,"directory":directory,"role":role.name(),
        "scope_id":"original","supervisor":supervisor,"monitor":monitor});
    let tree =
        json!({"operation_id":ID,"supervisor":supervisor,"monitor":monitor,"process":product});
    frame.match_tree(role, &directory, ID, &tree, &ready)?;
    for key in ["operation_id", "supervisor", "monitor", "process"] {
        let mut changed = tree.clone();
        changed[key] = Value::Null;
        assert!(
            frame
                .match_tree(role, &directory, ID, &changed, &ready)
                .is_err()
        );
    }
    let mut changed = ready;
    changed["scope_id"] = json!("replacement");
    assert!(
        frame
            .match_tree(role, &directory, ID, &tree, &changed)
            .is_err()
    );
    Ok(())
}

#[test]
fn an_archived_failure_with_a_registered_live_product_cannot_confirm_closure() -> Result<()> {
    let (_temporary, directory) = temporary()?;
    let role = Role::Worker;
    let product = process::current()?;
    let pid = product["pid"].as_u64().ok_or("缺少当前测试进程 PID")?;
    let supervisor =
        json!({"pid":pid+1,"started":"2","executable":directory.join("supervisor.exe")});
    let monitor = json!({"pid":pid+2,"started":"3","executable":directory.join("monitor.exe")});
    let runtime =
        json!({"scope_id":"original","artifacts":{"worker":{"path":product["executable"]}}});
    let frame = Frame {
        owner: Value::Null,
        request: json!({"scope_id":"original"}),
        supervisor: json!({"identity":supervisor}),
        progress: json!({"phase":"started","identity":product}),
        result: Value::Null,
    };
    let ready = json!({"format_version":1,"operation_id":ID,"directory":directory,"role":role.name(),
        "scope_id":"original","supervisor":supervisor,"monitor":monitor});
    let registered = json!({"format_version":2,"kind":"full-stack-process-tree","operation_id":ID,
        "runtime_directory":directory,"role":role.name(),"scope_id":"original","supervisor":supervisor,
        "monitor":monitor,"process":product,"group_id":supervisor["pid"]});
    operation::write_new(&tree::path(&directory, role, ID, "ready"), &ready)?;
    operation::write_new(&directory.join("worker-tree.json"), &registered)?;
    operation::write_new(
        &directory.join("worker.json"),
        &json!({"format_version":1,"role":role.name(),"scope_id":"original","identity":product}),
    )?;
    let closed = tree::path(&directory, role, ID, "stopped");
    operation::write_new(&closed, &json!({"status":"stopped"}))?;
    let error = frame
        .observe(role, &directory, &runtime, ID, "failed")
        .unwrap_err();
    assert!(error.to_string().contains("原产品仍存活"));
    assert_eq!(operation::read(&closed)?, json!({"status":"stopped"}));
    Ok(())
}

pub(crate) const STOP_ID_START: &str = "11111111111111111111111111111111";

pub(crate) fn stopped_runtime(directory: &Path, port: u16) -> Result<(Value, Value, Value)> {
    let api = directory.join("config");
    let worker = directory.join("worker-config");
    for (path, id) in [(&api, 3), (&worker, 2)] {
        fs::create_dir(path)?;
        fs::write(
            path.join("app.test.toml"),
            format!(
                "scope_id='archived-stop'\nsnowflake_worker_id={id}\n[jobs]\nmode='external'\nhealth_host='127.0.0.1'\nhealth_port={port}\n"
            ),
        )?;
    }
    let executable = directory.join("artifact.exe");
    fs::write(&executable, b"stopped fixture")?;
    operation::write_new(
        &directory.join("binaries.json"),
        &json!({"ryframe":executable,"ryframe-worker":executable,"ryframe-reset":executable,"ryframe-migrate":executable}),
    )?;
    let runtime = crate::recovery::fixture_control::artifact::runtime::contract(
        directory, directory, &api, &worker,
    )?;
    operation::write_new(&directory.join("runtime.json"), &runtime)?;
    let env = directory.join("environment.json");
    operation::write_new(&env, &json!({"environment":{"APP_ENV":"test"}}))?;
    let environment = common::binding(&env)?;
    let identity = |pid| json!({"pid":pid,"started":"1","executable":executable});
    let product = identity(u32::MAX - 11);
    let supervisor = identity(u32::MAX - 12);
    let monitor = identity(u32::MAX - 13);
    let executor = identity(u32::MAX - 14);
    let tree = json!({"format_version":2,"kind":"full-stack-process-tree",
        "runtime_directory":directory,"role":"worker","scope_id":runtime["scope_id"],
        "operation_id":STOP_ID_START,"supervisor":supervisor,"monitor":monitor,
        "process":product,"group_id":supervisor["pid"]});
    operation::write_new(&directory.join("worker-tree.json"), &tree)?;
    operation::write_new(
        &directory.join("worker.json"),
        &json!({"format_version":1,"role":"worker","scope_id":runtime["scope_id"],"identity":product}),
    )?;
    operation::write_new(
        &tree::path(directory, Role::Worker, STOP_ID_START, "stopped"),
        &json!({"format_version":1,"operation_id":STOP_ID_START,"directory":directory,
            "role":"worker","scope_id":runtime["scope_id"],"supervisor":supervisor,"monitor":monitor,
            "status":"stopped","members":[supervisor,product],"error_type":null}),
    )?;
    Ok((environment, product, executor))
}

pub(crate) fn archive_stopped_fixture(
    directory: &Path,
    executor: &Value,
    id: &str,
) -> Result<Value> {
    let guard = Guard::acquire(Role::Worker, directory)?;
    let (operation, expected) = recorded_stop_fixture(directory, executor, id)?;
    let archived = operation.archive()?;
    assert_eq!(archived, expected);
    guard.check()?;
    Ok(archived)
}

fn recorded_stop_fixture(
    directory: &Path,
    executor: &Value,
    id: &str,
) -> Result<(Operation, Value)> {
    let role = Role::Worker;
    let runtime = runtime::load(role, directory, directory)?;
    let op = Operation::claim_recorded_stop(role, directory, directory, &runtime, 1.0, id)?;
    assert_eq!(op.id()?, id);
    let mut owner = op.owner;
    owner["identity"] = executor.clone();
    let mut request = op.request;
    request["controller_identity"] = executor.clone();
    request["owner_sha256"] = json!(documents::hash(&owner)?);
    fs::write(op.path.join("owner.json"), serde_json::to_vec(&owner)?)?;
    fs::write(op.path.join("request.json"), serde_json::to_vec(&request)?)?;
    let op = Operation::read(role, directory, &operation::source(directory)?)?;
    let result = op.result("stopped", Value::Null, None, false)?;
    op.publish("result.json", &result, false)?;
    Ok((op, result))
}

#[test]
fn archived_stop_checks_exact_generation_and_closed_port_before_publishing() -> Result<()> {
    use std::{cell::Cell, net::TcpListener};

    let (_temporary, directory) = temporary()?;
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let (environment, product, executor) =
        stopped_runtime(&directory, listener.local_addr()?.port())?;
    let role = Role::Worker;
    let guard = Guard::acquire(role, &directory)?;
    let id = "22222222222222222222222222222222";
    let (op, result) = recorded_stop_fixture(&directory, &executor, id)?;
    let pending = fs::read(op.path.join("result.json"))?;
    drop(guard);
    let different = ArchivedStop {
        executor: &executor,
        id,
        generation: StopGeneration {
            start_id: ID,
            identity: &product,
        },
    };
    let error = super::super::reconcile_recorded_stop(
        role,
        &directory,
        &directory,
        &environment,
        &different,
    )
    .unwrap_err();
    assert!(error.to_string().contains("不同于已预留的启动代次"));
    assert_eq!(fs::read(op.path.join("result.json"))?, pending);
    assert!(!directory.join(role.archive()).join(id).exists());
    let guard = Guard::acquire(role, &directory)?;
    assert_eq!(op.archive()?, result);
    drop(guard);
    let archive = directory.join(role.archive()).join(id);
    let before = fs::read(archive.join("result.json"))?;
    let count = Cell::new(0);
    let observe = |id: &str, start_id: &str, identity: &Value, executor: &Value| {
        archived_stop_bound(
            role,
            &directory,
            &directory,
            &environment,
            &ArchivedStop {
                executor,
                id,
                generation: StopGeneration { start_id, identity },
            },
            || {
                count.set(count.get() + 1);
                Ok(())
            },
        )
    };
    assert!(observe(id, STOP_ID_START, &product, &executor).is_err());
    assert_eq!(count.get(), 0);
    drop(listener);
    assert!(observe(ID, STOP_ID_START, &product, &executor)?.is_none());
    assert!(observe(id, ID, &product, &executor).is_err());
    assert!(observe(id, STOP_ID_START, &executor, &executor).is_err());
    assert!(observe(id, STOP_ID_START, &product, &product).is_err());
    assert_eq!(count.get(), 0);
    assert_eq!(
        observe(id, STOP_ID_START, &product, &executor)?,
        Some(result)
    );
    assert_eq!(count.get(), 1);
    assert_eq!(fs::read(archive.join("result.json"))?, before);
    assert!(!directory.join(role.lock()).exists());
    Ok(())
}

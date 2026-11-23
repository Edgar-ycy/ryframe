use super::*;
use crate::recovery::{clone::native::json::write_new, runtime::REGISTRATION_LOCK};
use std::{cell::Cell, fs};

struct Fixture {
    _directory: tempfile::TempDir,
    backend: std::path::PathBuf,
    control: std::path::PathBuf,
    registration: std::path::PathBuf,
    binding: Value,
}

impl Fixture {
    fn new() -> Self {
        let local = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("restore-guard-")
            .tempdir_in(local)
            .unwrap();
        let backend = registration::protocol_path(directory.path()).unwrap();
        let control = backend.join(".local-tests");
        let evidence = control.join("evidence");
        fs::create_dir_all(&evidence).unwrap();
        let reference = evidence.join("reference.json");
        let target = evidence.join("target.json");
        write_new(&reference, &json!({"fixture":"reference"})).unwrap();
        write_new(&target, &json!({"fixture":"target"})).unwrap();
        let reference = document::read(&reference, &backend, "参考夹具").unwrap();
        let target = document::read(&target, &backend, "目标夹具").unwrap();
        let registration = evidence.join("registration.json");
        let configuration =
            super::super::dataset_authority::configuration_registration::sidecar(&registration)
                .unwrap();
        fs::create_dir(&configuration).unwrap();
        let environment = configuration.join("environment.json");
        write_new(&environment, &json!({"fixture":"runtime-environment"})).unwrap();
        let environment = document::read(&environment, &backend, "运行环境夹具").unwrap();
        write_new(&registration, &json!({
            "format_version":2,"kind":"restore-runtime-registration","remote_writes":0,
            "reference_plan":document::binding(&reference),"target_plan":document::binding(&target),
            "runtime_environment":document::binding(&environment),
            "observation":{"runtime":{"exists":false,"entries":[],"anchor":{
                "path":evidence,"device":1,"inode":1,"created_ns":1}},
                "ports_idle":["api","worker","frontend"]},
        })).unwrap();
        let saved = document::read(&registration, &backend, "登记夹具").unwrap();
        let binding = json!({"registration":document::binding(&saved),"target_plan":document::binding(&target)});
        Self {
            _directory: directory,
            backend,
            control,
            registration,
            binding,
        }
    }
}

#[test]
fn registered_inputs_require_the_exact_target_descriptor() {
    let fixture = Fixture::new();
    let read = |binding: &Value| {
        Bound::read(
            &fixture.backend,
            &fixture.registration,
            binding,
            &mut Inputs::new(&fixture.backend),
        )
    };
    assert_eq!(
        read(&fixture.binding["target_plan"]).unwrap().binding,
        fixture.binding
    );
    let mut wrong = fixture.binding["target_plan"].clone();
    wrong["sha256"] = json!("0".repeat(64));
    assert!(read(&wrong).is_err());
    let foreign = fixture.control.join("evidence/foreign-target.json");
    fs::copy(fixture.control.join("evidence/target.json"), &foreign).unwrap();
    let document = document::read(&foreign, &fixture.backend, "另一目标文件").unwrap();
    assert!(read(&document::binding(&document)).is_err());
    assert!(!fixture.control.join(REGISTRATION_LOCK).exists());
}

#[test]
fn independent_stopped_runtime_refuses_an_ordinary_registered_reference() -> Result<()> {
    let fixture = Fixture::new();
    let mut inputs = Inputs::new(&fixture.backend);
    let bound = Bound::read(
        &fixture.backend,
        &fixture.registration,
        &fixture.binding["target_plan"],
        &mut inputs,
    )?;
    let error = bound.check_independent(&fixture.backend, &inputs, None, &|_, _| {
        panic!("普通登记不能调用独立工作目录归属回调")
    });
    assert_eq!(
        error.unwrap_err().to_string(),
        "独立 B1 开始登记必须使用原独立参考计划"
    );
    assert!(!crate::recovery::runtime::dataset_authority::reads::is_active());
    assert!(!fixture.control.join(REGISTRATION_LOCK).exists());
    Ok(())
}

#[test]
fn checkpoints_hold_the_exact_restore_owner_and_repeat_on_exit() {
    let fixture = Fixture::new();
    let owner_path = fixture.control.join(REGISTRATION_LOCK).join("owner.json");
    let calls = Cell::new(0);
    let reply = with_checked_owner(
        &fixture.control,
        &fixture.binding,
        |owner, checks| {
            assert!(checks.backup.is_none());
            assert!(checks.source.is_none());
            owner.check()?;
            let receipt: Value = serde_json::from_slice(&fs::read(&owner_path)?)?;
            let registration = fixture.binding["registration"]["sha256"].as_str().unwrap();
            let target = fixture.binding["target_plan"]["sha256"].as_str().unwrap();
            assert_eq!(
                receipt["operation"],
                format!("restore:{registration}:{target}")
            );
            calls.set(calls.get() + 1);
            Ok(())
        },
        |checkpoint| {
            let foreign: Result<()> =
                Owner::with(&fixture.control, "runtime-start:fixture", |_| {
                    panic!("恢复期间不能接管 owner")
                });
            assert!(foreign.is_err());
            checkpoint(RestoreChecks::default())
        },
    )
    .unwrap();
    assert_eq!(calls.get(), 3);
    assert_eq!(reply, fixture.binding);
    assert!(!owner_path.exists());
}

#[test]
fn independent_stopped_runtime_rechecks_the_same_owner_before_during_and_after() -> Result<()> {
    let fixture = Fixture::new();
    let owner_path = fixture.control.join(REGISTRATION_LOCK).join("owner.json");
    let owner_bytes = std::cell::RefCell::new(None);
    let checks = Cell::new(0);
    let result = with_independent_owner(
        &fixture.control,
        &fixture.binding,
        |owner| {
            owner.check()?;
            let bytes = fs::read(&owner_path)?;
            let mut previous = owner_bytes.borrow_mut();
            if let Some(expected) = previous.as_ref() {
                assert_eq!(&bytes, expected);
            } else {
                let receipt: Value = serde_json::from_slice(&bytes)?;
                let registration = fixture.binding["registration"]["sha256"].as_str().unwrap();
                let target = fixture.binding["target_plan"]["sha256"].as_str().unwrap();
                assert_eq!(
                    receipt["operation"],
                    format!("restore:{registration}:{target}")
                );
                *previous = Some(bytes);
            }
            checks.set(checks.get() + 1);
            Ok(())
        },
        |checkpoint| {
            assert_eq!(checks.get(), 1);
            assert_eq!(checkpoint()?, fixture.binding);
            assert_eq!(checks.get(), 2);
            Ok("完成")
        },
    )?;
    assert_eq!(result, "完成");
    assert_eq!(checks.get(), 3);
    assert!(!owner_path.exists());
    Ok(())
}

#[test]
fn precheck_and_exit_failure_preserve_original_failure_without_replay() {
    let fixture = Fixture::new();
    let first = with_checked_owner(
        &fixture.control,
        &fixture.binding,
        |_, _| Err("原登记漂移".into()),
        |_| -> Result<()> { panic!("首次核验失败不能进入正文") },
    )
    .unwrap_err();
    assert_eq!(first.to_string(), "原登记漂移");
    let calls = Cell::new(0);
    let failure = with_checked_owner(
        &fixture.control,
        &fixture.binding,
        |_, _| {
            let call = calls.replace(calls.get() + 1);
            if call == 0 {
                Ok(())
            } else {
                Err("私有探测诊断".into())
            }
        },
        |_| -> Result<()> { Err("原恢复写失败".into()) },
    )
    .unwrap_err();
    assert_eq!(calls.get(), 2);
    assert_eq!(failure.source().unwrap().to_string(), "原恢复写失败");
    assert!(failure.to_string().contains("零进程状态复核也失败"));
    assert!(!failure.to_string().contains("私有探测诊断"));
    assert!(!fixture.control.join(REGISTRATION_LOCK).exists());
}

#[test]
fn successful_restore_body_cannot_leave_an_unregistered_runtime_file() {
    let fixture = Fixture::new();
    let runtime = fixture.control.join("evidence/runtime");
    let foreign = runtime.join("foreign.json");
    let payload = json!({"fixture":"unregistered runtime file"});
    let (_, registered) = registration::runtime_observation(&fixture.backend, &runtime).unwrap();
    assert_eq!(registered["exists"], false);
    let owner_path = fixture.control.join(REGISTRATION_LOCK).join("owner.json");
    let checks = Cell::new(0);
    let writes = Cell::new(0);

    let error = with_checked_owner(
        &fixture.control,
        &fixture.binding,
        |owner, _| {
            owner.check()?;
            checks.set(checks.get() + 1);
            let (_, observed) = registration::runtime_observation(&fixture.backend, &runtime)?;
            assert_eq!(observed, registered);
            Ok(())
        },
        |_| {
            assert!(owner_path.is_file());
            let contender: Result<()> =
                Owner::with(&fixture.control, "runtime-start:fixture", |_| {
                    panic!("恢复正文持有登记锁时不能进入另一操作")
                });
            assert!(contender.is_err());
            writes.set(writes.get() + 1);
            fs::create_dir(&runtime)?;
            write_new(&foreign, &payload)?;
            Ok(())
        },
    )
    .unwrap_err();

    assert!(error.to_string().contains("fresh target 运行目录已出现"));
    assert_eq!(checks.get(), 2);
    assert_eq!(writes.get(), 1);
    assert_eq!(
        fs::read(&foreign).unwrap(),
        document::canonical_bytes(&payload).unwrap()
    );
    assert!(!fixture.control.join(REGISTRATION_LOCK).exists());
}

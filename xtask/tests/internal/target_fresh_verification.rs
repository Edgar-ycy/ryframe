//! 共用只读复验窗口：明确 E、完整初像、父收尾与观察拓扑。
use super::{
    execute,
    fixture::{Resources, retain_prepared},
};
use crate::{
    Result,
    recovery::clone::native::{
        bridge::scope::Scope,
        json as document,
        resources::common,
        target_verify::{fresh::verify, guard::Generation, published, storage::StorageInputs},
    },
    target_verify_fixture::{Fixture, snapshot},
};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn observed(
    resources: &Resources<'_>,
    observation: &Path,
) -> Result<(
    Value,
    crate::recovery::clone::native::target_verify::guard::Inputs,
)> {
    let f = resources.fixture;
    let owner = Generation::acquire(&f.backend, &f.target)?;
    let result = (|| {
        let initialized = published::evidence_history(
            &f.backend,
            &f.target.join("initialized.json"),
            Some(&owner),
        )?
        .0;
        let descriptor = common::binding(&f.target.join("initialized.json"))?;
        let mut environment = f.environment.clone();
        environment.insert("TEMP".into(), f.local.to_str().unwrap().into());
        let generation = || resources.generation_at(observation);
        let metadata = fs::symlink_metadata(&f.local)?;
        let (device, inode) = common::device_inode_for(&f.local, &metadata)?;
        let temporary = json!({"path":f.local,"device":device,"inode":inode});
        verify::window::Window {
            inputs: StorageInputs {
                backend: &f.backend,
                request: &f.request,
                environment: &environment,
                observation_dir: observation,
            },
            target: &f.target,
            descriptor: &descriptor,
            initialized: &initialized,
            owner: &owner,
            transport: resources,
            observer: resources,
            generation: &generation,
            temporary: &temporary,
        }
        .observe()
    })();
    owner.finish(result)
}

#[test]
fn fresh_verification_preserves_initialized_tree_and_rejects_parent_source_drift_before_publication()
 {
    let f = Fixture::for_initialization();
    let resources = Resources::new(&f);
    retain_prepared(&f);
    execute(&resources, false).unwrap();
    let original = snapshot(&f.target);
    let commands = resources.calls.borrow().len();
    let redis = resources.redis_calls.borrow().len();
    let observation = f.local.join("readonly-verify");
    fs::create_dir(&observation).unwrap();
    let (candidate, protected) = observed(&resources, &observation).unwrap();
    assert!(!observation.join("verify.json").exists());
    protected.check().unwrap();
    let value = verify::finish(&observation, Ok(candidate)).unwrap();
    assert_eq!(value["remote_writes"], 0);
    assert_eq!(
        value["initialized"],
        common::binding(&f.target.join("initialized.json")).unwrap()
    );
    assert_eq!(
        value["inventory"]["observations"],
        document::read(&f.target.join("initialized.json")).unwrap()["inventory"]["observations"]
    );
    assert_eq!(snapshot(&f.target), original);
    // 真实库存替身同时断言每个维护命令使用 request.config_dir，backend/config 为非法诱饵。
    assert!(
        resources.calls.borrow()[commands..]
            .iter()
            .all(|args| args.iter().any(|v| v == "target-inventory")
                || args.iter().any(|v| v == "--execute"))
    );
    assert!(
        resources.redis_calls.borrow()[redis..]
            .iter()
            .all(|args| matches!(args[0].as_str(), "SCAN" | "GET"))
    );
    drop(protected);

    let scope = Scope::load(&f.backend, &[], &[]).unwrap();
    let failed = f.local.join("parent-drift-verify");
    fs::create_dir(&failed).unwrap();
    let (candidate, protected) = observed(&resources, &failed).unwrap();
    let source = f.backend.join("Cargo.toml");
    let bytes = fs::read(&source).unwrap();
    let mut changed = bytes.clone();
    changed.extend_from_slice(b"\n# parent source changed after resource observation\n");
    fs::write(&source, changed).unwrap();
    let result = scope.assert_unchanged().and_then(|()| {
        protected.check()?;
        Ok(candidate)
    });
    assert!(verify::finish(&failed, result).is_err());
    assert!(!failed.join("verify.json").exists());
    assert_eq!(
        document::read(&failed.join("failure.json")).unwrap()["stage"],
        "verify_existing"
    );
    assert_eq!(snapshot(&f.target), original);
    fs::write(source, bytes).unwrap();
}

#[test]
fn fresh_verification_private_boundary_rejects_overlap_trailing_separator_and_extra_arguments_without_writes()
 {
    let f = Fixture::for_status();
    let original = snapshot(&f.target);
    let descriptor = common::binding(&f.target.join("initialized.json")).unwrap();
    let trailing = format!("{}{}", f.run.display(), std::path::MAIN_SEPARATOR);
    let args = json!({"initialized":descriptor,"observation":f.run.join("nested-observation"),
        "storage_run":trailing,"temporary_directory":f.local,"app_env":"test","write":true});
    let invoke = |arguments: Value| verify::binding(&f.backend, &arguments);
    assert!(invoke(args.clone()).is_err());
    assert!(!f.run.join("nested-observation").exists());
    let mut extra = args.clone();
    extra["config_dir"] = json!(f.local.join("config"));
    assert!(invoke(extra).is_err());
    #[cfg(windows)]
    let mut folded = args.clone();
    let mut invalid_temporary = args;
    invalid_temporary["storage_run"] = Value::Null;
    invalid_temporary["observation"] = json!(f.local.join("unused-temporary-observation"));
    for path in [
        Path::new("relative").to_path_buf(),
        f.backend.clone(),
        f.local.join("missing-tmp"),
    ] {
        invalid_temporary["temporary_directory"] = json!(path);
        assert!(invoke(invalid_temporary.clone()).is_err());
        assert!(!f.local.join("unused-temporary-observation").exists());
    }
    #[cfg(windows)]
    {
        folded["storage_run"] = json!(trailing.to_uppercase());
        assert!(invoke(folded).is_err());
        assert!(!f.run.join("nested-observation").exists());
    }
    assert_eq!(snapshot(&f.target), original);
}

#[test]
fn fresh_verification_direct_target_needs_no_workspace_and_rejects_incomplete_inventory() {
    let f = Fixture::new();
    // 直接观察从目标目录定位原登记；两份外层快照属于正式工作区入口。
    assert!(f.local.join("registration.json").is_file());
    for name in ["prepared-files.json", "initialized-files.json"] {
        let path = f.local.join(name);
        fs::remove_file(&path).unwrap();
        assert!(!path.exists());
    }
    let resources = Resources::new(&f);
    resources.initialized_state();
    let original = snapshot(&f.target);
    let observation = f.local.join("direct-verification");
    fs::create_dir(&observation).unwrap();
    let (candidate, protected) = observed(&resources, &observation).unwrap();
    protected.check().unwrap();
    verify::finish(&observation, Ok(candidate)).unwrap();
    drop(protected);
    assert_eq!(snapshot(&f.target), original);

    resources.fail.set(Some("inventory-model"));
    let failed = f.local.join("direct-inventory-failure");
    fs::create_dir(&failed).unwrap();
    let result = observed(&resources, &failed);
    assert!(result.is_err());
    let result = result.map(|(value, _)| value);
    assert!(verify::finish(&failed, result).is_err());
    assert!(!failed.join("verify.json").exists());
    assert_eq!(
        document::read(&failed.join("failure.json")).unwrap()["error_type"],
        "InventoryCaptureError"
    );
    assert_eq!(snapshot(&f.target), original);
}

#[test]
fn fresh_verification_formal_wrapper_rejects_invalid_release_snapshot_before_resource_observation()
{
    let f = Fixture::for_status();
    let original = snapshot(&f.target);
    let path = f.local.join("initialized-files.json");
    let bytes = fs::read(&path).unwrap();
    let released = document::read(&path).unwrap();
    for (index, (field, value)) in [
        ("kind", json!("invalid-release-snapshot")),
        ("registration", Value::Null),
        ("predecessor", Value::Null),
        ("files", json!([])),
    ]
    .into_iter()
    .enumerate()
    {
        let mut invalid = released.clone();
        invalid[field] = value;
        fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
        let observation = f
            .backend
            .join(".local-tests")
            .join(format!("invalid-release-{index}"));
        let error = verify::execute(&f.backend, &f.local, &observation).unwrap_err();
        assert_eq!(error.to_string(), "fresh 阶段文件集合、内容或登记发生变化");
        assert!(!observation.exists());
        assert_eq!(snapshot(&f.target), original);
    }
    fs::write(path, bytes).unwrap();
}

use super::*;
use crate::recovery::fixture_control::{
    environment::plan::CheckedPlan, source_pair::execution::tests::Fixture,
};
use std::{fs, io::ErrorKind, path::PathBuf};

#[test]
fn checked_plan_preserves_original_projection_and_hash_boundaries() {
    let fixture = Fixture::new();
    let (root, plan) = published_plan(&fixture);
    let checked = CheckedPlan::load(&fixture.backend, &plan).unwrap();
    let projection = checked.projection();
    let review = crate::recovery::fixture_control::portable_path(
        &root.join("review.json").canonicalize().unwrap(),
    );
    let maintenance = crate::recovery::fixture_control::portable_path(
        &root.join("maintenance_build.json").canonicalize().unwrap(),
    );
    assert_eq!(projection["inputs"][0], json!(review));
    assert_eq!(
        projection["inputs"][1],
        crate::recovery::clone::native::json::read(&review).unwrap()
    );
    assert_eq!(projection["inputs"][2], plan["execution_source"]["path"]);
    assert_eq!(
        projection["inputs"][3]["published"]["backend"]["path"],
        plan["execution_backend"]
    );
    assert_eq!(projection["inputs"][4], json!(maintenance));
    assert_eq!(
        projection["inputs"][5],
        crate::recovery::clone::native::json::read(&maintenance).unwrap()
    );
    assert_eq!(projection["inputs"].as_array().unwrap().len(), 6);
    checked.verify().unwrap();
    assert_eq!(checked.projection(), projection);
    let mut unsigned = plan.clone();
    unsigned.as_object_mut().unwrap().remove("sha256");
    assert_eq!(
        plan["sha256"],
        crate::recovery::clone::native::digest::plan_hash(&unsigned).unwrap()
    );
}

#[test]
fn checked_plan_rejects_input_bytes_identity_and_source_drift_within_one_window() {
    let fixture = Fixture::new();
    let (root, plan) = published_plan(&fixture);
    let inputs = [
        root.join("review.json"),
        fixture.output.join("execution-source.json"),
        root.join("maintenance_build.json"),
    ];
    for path in inputs {
        let original = fs::read(&path).unwrap();
        let checked = CheckedPlan::load(&fixture.backend, &plan).unwrap();
        match fs::write(&path, b"{}") {
            Ok(()) => assert!(checked.verify().is_err()),
            Err(error) => {
                assert!(
                    error.kind() == ErrorKind::PermissionDenied
                        || (cfg!(windows) && matches!(error.raw_os_error(), Some(32 | 33))),
                    "受保护文件写入出现非预期错误：{error}; kind={:?}, raw_os_error={:?}",
                    error.kind(),
                    error.raw_os_error()
                );
                assert_eq!(fs::read(&path).unwrap(), original);
                checked.verify().unwrap();
            }
        }
        drop(checked);
        fs::write(&path, original).unwrap();
    }
    let review = root.join("review.json");
    let retained = root.join("review-retained.json");
    let original = fs::read(&review).unwrap();
    let checked = CheckedPlan::load(&fixture.backend, &plan).unwrap();
    let replaced = match fs::rename(&review, &retained) {
        Ok(()) => {
            fs::write(&review, &original).unwrap();
            assert!(checked.verify().is_err());
            true
        }
        Err(error) => {
            assert!(
                error.kind() == ErrorKind::PermissionDenied
                    || (cfg!(windows) && matches!(error.raw_os_error(), Some(32 | 33))),
                "受保护文件替换出现非预期错误：{error}; kind={:?}, raw_os_error={:?}",
                error.kind(),
                error.raw_os_error()
            );
            checked.verify().unwrap();
            false
        }
    };
    drop(checked);
    if replaced {
        fs::remove_file(&review).unwrap();
        fs::rename(&retained, &review).unwrap();
    }
    let checked = CheckedPlan::load(&fixture.backend, &plan).unwrap();
    let source = PathBuf::from(plan["execution_backend"].as_str().unwrap()).join("src/lib.rs");
    let before = fs::read(&source).unwrap();
    fs::write(&source, "// 源码漂移\n").unwrap();
    assert!(checked.verify().is_err());
    drop(checked);
    fs::write(source, before).unwrap();
}

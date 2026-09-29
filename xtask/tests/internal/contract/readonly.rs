use super::*;
use crate::contract::check_current_with;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    process::Command,
};

fn git(frontend: &TestFrontend, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(frontend.backend())
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn generated_content(relative: &str) -> Vec<u8> {
    if relative == "src/api/generated/ownership.json" {
        serde_json::to_vec(
            &serde_json::json!({"version": 1, "files": &CANDIDATE_MANAGED_PATHS[1..]}),
        )
        .unwrap()
    } else {
        format!("generated:{relative}").into_bytes()
    }
}

fn prepare_readonly_fixture(frontend: &TestFrontend) {
    fs::write(frontend.backend().join("openapi/openapi.json"), candidate()).unwrap();
    fs::write(frontend.0.join("openapi/openapi.json"), candidate()).unwrap();
    git(frontend, &["init", "--quiet", "--initial-branch=main"]);
    git(frontend, &["config", "core.autocrlf", "false"]);
    git(frontend, &["add", "openapi/openapi.json"]);
    git(
        frontend,
        &[
            "-c",
            "user.name=RyFrame CI",
            "-c",
            "user.email=ci@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "--quiet",
            "-m",
            "contract",
        ],
    );
    let commit = git(frontend, &["rev-parse", "HEAD"]);
    let metadata = serde_json::json!({
        "schema_version": 1,
        "backend_repository": "Edgar-ycy/ryframe",
        "backend_commit": commit,
        "openapi_path": "openapi/openapi.json",
        "openapi_version": "3.1.0",
        "sha256": sha256_hex(candidate()),
    });
    let mut metadata = serde_json::to_string_pretty(&metadata)
        .unwrap()
        .into_bytes();
    metadata.push(b'\n');
    fs::write(frontend.0.join("openapi/source.json"), metadata).unwrap();
    let _ = fs::remove_file(frontend.0.join("openapi/candidate.json"));
    for relative in &CANDIDATE_MANAGED_PATHS[1..] {
        fs::write(frontend.0.join(relative), generated_content(relative)).unwrap();
    }
    fs::create_dir_all(frontend.0.join(".local-tests")).unwrap();
}

fn write_generated_artifacts(staging: &Path) -> crate::Result<()> {
    for relative in &CANDIDATE_MANAGED_PATHS[1..] {
        fs::write(staging.join(relative), generated_content(relative))?;
    }
    Ok(())
}

fn controlled_tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn collect(root: &Path, current: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(current).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap();
            if relative.starts_with(".local-tests")
                || relative.starts_with("backend/target")
                || relative.starts_with("backend/.git")
            {
                continue;
            }
            if entry.file_type().unwrap().is_dir() {
                collect(root, &path, result);
            } else {
                result.insert(relative.to_path_buf(), fs::read(path).unwrap());
            }
        }
    }
    let mut result = BTreeMap::new();
    collect(root, root, &mut result);
    result
}

fn contract_staging_directories(frontend: &TestFrontend) -> Vec<PathBuf> {
    fs::read_dir(frontend.0.join(".local-tests"))
        .unwrap()
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("contract-stage-"))
        })
        .collect()
}

#[test]
fn readonly_api_generation_compares_current_backend_and_all_frontend_artifacts() {
    let frontend = TestFrontend::new();
    prepare_readonly_fixture(&frontend);
    let before = controlled_tree(&frontend.0);
    let exported = Cell::new(false);
    let generated = Cell::new(false);
    let export_path = RefCell::new(None::<PathBuf>);

    check_current_with(
        &frontend.backend(),
        &frontend.0,
        |output| {
            exported.set(true);
            export_path.replace(Some(output.to_path_buf()));
            fs::write(output, candidate())?;
            Ok(())
        },
        |staging| {
            generated.set(true);
            write_generated_artifacts(staging)
        },
    )
    .unwrap();

    assert!(exported.get() && generated.get());
    assert_eq!(controlled_tree(&frontend.0), before);
    assert!(contract_staging_directories(&frontend).is_empty());
    assert!(
        !export_path
            .borrow()
            .as_ref()
            .unwrap()
            .parent()
            .unwrap()
            .exists()
    );
}

#[test]
fn readonly_api_generation_rejects_backend_fact_drift_without_running_generator() {
    let frontend = TestFrontend::new();
    prepare_readonly_fixture(&frontend);
    fs::write(
        frontend.backend().join("openapi/openapi.json"),
        b"{\"openapi\":\"3.1.0\",\"info\":{\"title\":\"RyFrame API\"}}\n",
    )
    .unwrap();
    let generated = Cell::new(false);

    let error = check_current_with(
        &frontend.backend(),
        &frontend.0,
        |output| {
            fs::write(output, candidate())?;
            Ok(())
        },
        |_| {
            generated.set(true);
            Ok(())
        },
    )
    .unwrap_err()
    .to_string();

    assert!(error.contains("后端提交的 openapi/openapi.json"), "{error}");
    assert!(!generated.get());
    assert!(contract_staging_directories(&frontend).is_empty());
}

#[test]
fn readonly_api_generation_cleans_temporary_paths_after_export_or_generation_failure() {
    let frontend = TestFrontend::new();
    prepare_readonly_fixture(&frontend);
    let export_path = RefCell::new(None::<PathBuf>);
    let error = check_current_with(
        &frontend.backend(),
        &frontend.0,
        |output| {
            export_path.replace(Some(output.to_path_buf()));
            Err("模拟 OpenAPI 导出失败".into())
        },
        |_| Ok(()),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("模拟 OpenAPI 导出失败"));
    assert!(
        !export_path
            .borrow()
            .as_ref()
            .unwrap()
            .parent()
            .unwrap()
            .exists()
    );

    let stage_path = RefCell::new(None::<PathBuf>);
    let error = check_current_with(
        &frontend.backend(),
        &frontend.0,
        |output| {
            fs::write(output, candidate())?;
            Ok(())
        },
        |staging| {
            stage_path.replace(Some(staging.to_path_buf()));
            Err("模拟前端派生失败".into())
        },
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("模拟前端派生失败"));
    assert!(!stage_path.borrow().as_ref().unwrap().exists());
    assert!(contract_staging_directories(&frontend).is_empty());
}

#[test]
fn readonly_api_generation_rejects_derived_drift_and_concurrent_input_changes() {
    let frontend = TestFrontend::new();
    prepare_readonly_fixture(&frontend);
    let drifted = frontend.0.join(CANDIDATE_MANAGED_PATHS[1]);
    fs::write(&drifted, "manual-drift").unwrap();
    let error = check_current_with(
        &frontend.backend(),
        &frontend.0,
        |output| {
            fs::write(output, candidate())?;
            Ok(())
        },
        write_generated_artifacts,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains(CANDIDATE_MANAGED_PATHS[1]), "{error}");
    assert!(contract_staging_directories(&frontend).is_empty());

    prepare_readonly_fixture(&frontend);
    let manifest = frontend.0.join("scripts/api-artifacts.mjs");
    let error = check_current_with(
        &frontend.backend(),
        &frontend.0,
        |output| {
            fs::write(output, candidate())?;
            Ok(())
        },
        |staging| {
            write_generated_artifacts(staging)?;
            fs::write(
                &manifest,
                "export const generatedArtifactPaths = Object.freeze([])\n",
            )?;
            Ok(())
        },
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("契约生成输入在事务期间发生变化"), "{error}");
    assert!(contract_staging_directories(&frontend).is_empty());
}

#[test]
fn readonly_api_generation_rejects_unbound_source_metadata() {
    for field in ["backend_repository", "backend_commit", "extra", "duplicate"] {
        let frontend = TestFrontend::new();
        prepare_readonly_fixture(&frontend);
        let path = frontend.0.join("openapi/source.json");
        let mut source: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        match field {
            "backend_repository" => source[field] = "other/repository".into(),
            "backend_commit" => source[field] = "f".repeat(40).into(),
            "extra" => source[field] = true.into(),
            _ => {}
        }
        let bytes = if field == "duplicate" {
            format!(
                "{{\"schema_version\":1,{}",
                serde_json::to_string(&source)
                    .unwrap()
                    .trim_start_matches('{')
            )
        } else {
            serde_json::to_string(&source).unwrap()
        };
        fs::write(&path, bytes).unwrap();
        let generated = Cell::new(false);
        let result = check_current_with(
            &frontend.backend(),
            &frontend.0,
            |output| {
                fs::write(output, candidate())?;
                Ok(())
            },
            |_| {
                generated.set(true);
                Ok(())
            },
        );
        assert!(result.is_err(), "accepted invalid field {field}");
        assert!(!generated.get());
        assert!(contract_staging_directories(&frontend).is_empty());
    }
}

#[test]
fn readonly_api_generation_detects_obsolete_owned_artifact_without_deleting_source() {
    let frontend = TestFrontend::new();
    prepare_readonly_fixture(&frontend);
    let stale = "src/api/generated/obsolete.ts";
    fs::write(frontend.0.join(stale), "stale").unwrap();
    let ownership = frontend.0.join("src/api/generated/ownership.json");
    let mut document: serde_json::Value =
        serde_json::from_slice(&fs::read(&ownership).unwrap()).unwrap();
    document["files"].as_array_mut().unwrap().push(stale.into());
    fs::write(&ownership, serde_json::to_vec(&document).unwrap()).unwrap();
    let before = controlled_tree(&frontend.0);
    let error = check_current_with(
        &frontend.backend(),
        &frontend.0,
        |output| {
            fs::write(output, candidate())?;
            Ok(())
        },
        |staging| {
            write_generated_artifacts(staging)?;
            fs::remove_file(staging.join(stale))?;
            Ok(())
        },
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("派生文件发生漂移"), "{error}");
    assert_eq!(controlled_tree(&frontend.0), before);
    assert!(contract_staging_directories(&frontend).is_empty());
}

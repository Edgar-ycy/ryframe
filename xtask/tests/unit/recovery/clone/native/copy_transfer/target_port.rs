use std::fs;

use super::{history::verify_fixture_generation, *};

fn workspace() -> Result<(tempfile::TempDir, PathBuf)> {
    let local = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&local)?;
    let directory = tempfile::tempdir_in(local)?;
    let root = request_configuration::canonical_path(directory.path())?;
    Ok((directory, root))
}

#[test]
fn copy_target_temporary_requires_the_registered_existing_local_directory() -> Result<()> {
    let (_directory, root) = workspace()?;
    let local = root.join(".local-tests");
    let first = local.join("first");
    let other = local.join("other");
    fs::create_dir_all(&first)?;
    fs::create_dir(&other)?;
    let mut environment = BTreeMap::from([
        ("APP_ENV".into(), "test".into()),
        ("TEMP".into(), first.to_string_lossy().into_owned()),
        ("TMP".into(), first.to_string_lossy().into_owned()),
    ]);
    let (selected, identity) = temporary(&root, &environment)?;
    assert_eq!(selected, first);
    assert_eq!(common::directory_identity(&root, &identity)?, selected);

    environment.insert("TMP".into(), other.to_string_lossy().into_owned());
    assert!(temporary(&root, &environment).is_err());
    let missing = local.join("missing");
    environment.insert("TEMP".into(), missing.to_string_lossy().into_owned());
    environment.insert("TMP".into(), missing.to_string_lossy().into_owned());
    assert!(temporary(&root, &environment).is_err());
    assert!(!missing.exists());
    Ok(())
}

#[test]
fn copy_target_objects_bind_original_test_toml_and_only_return_public_facts() -> Result<()> {
    let (_directory, root) = workspace()?;
    let config = root.join("config");
    fs::create_dir(&config)?;
    let file = config.join("app.test.toml");
    fs::write(
        &file,
        "[object_storage]\nendpoint='http://127.0.0.1:19010'\nregion='local'\naccess_key='private-access'\nsecret_key='private-secret'\n",
    )?;
    let environment = BTreeMap::from([("APP_ENV".into(), "test".into())]);
    let (directory, files) = request_configuration::configuration_files(&root, &environment, None)?;
    let mut request = json!({
        "config_dir":config,
        "configuration_sha256":request_configuration::configuration_identity(&directory,&files)?,
        "target":{"s3":{"endpoint":"http://127.0.0.1:19010","region":"local"}},
    });
    let inspect = |request: &Value| {
        object_binding(
            &StorageInputs {
                backend: &root,
                request,
                environment: &environment,
                observation_dir: &root,
            },
            &root,
        )
    };
    let (binding, endpoint, region) = inspect(&request)?;
    assert_eq!(endpoint, "http://127.0.0.1:19010");
    assert_eq!(region, "local");
    assert!(!binding.to_string().contains("private-"));
    request["target"]["s3"]["region"] = json!("other");
    assert!(inspect(&request).is_err());
    request["target"]["s3"]["region"] = json!("local");
    fs::write(&file, "[object_storage]\nregion='local'\n")?;
    assert!(inspect(&request).is_err());
    Ok(())
}

#[test]
fn reference_uses_the_existing_observation_directory_without_a_copy_directory() -> Result<()> {
    let (_directory, root) = workspace()?;
    let local = root.join(".local-tests");
    let observation = local.join("reference-observation");
    let copy = local.join("unused-copy");
    fs::create_dir_all(&observation)?;
    let manifest = json!({"copy_stage":"seed_to_arm","copy_directory":copy});
    let (selected, identity) = reference_observation(&root, &manifest, &observation)?;
    assert_eq!(selected, observation);
    assert_eq!(common::directory_identity(&root, &identity)?, observation);
    assert!(!copy.exists());

    let missing = local.join("missing-reference-observation");
    assert!(reference_observation(&root, &manifest, &missing).is_err());
    assert!(!missing.exists());
    assert!(
        reference_observation(&root, &json!({"copy_stage":"source_to_seed"}), &observation)
            .is_err()
    );
    Ok(())
}

#[test]
fn independent_target_uses_original_history_and_the_locked_storage_state() -> Result<()> {
    let fixture = crate::target_verify_fixture::Fixture::for_status();
    let root = &fixture.backend;
    let registration = fixture.local.join("registration.json");
    let registered = document::read(&registration)?;
    let original = &registered["storage_run"];
    let current = fixture.local.join("independent-reference");
    fs::create_dir(&current)?;
    let (observed, identity) = reference_directory(root, &current)?;
    assert_eq!(observed, current);
    assert_eq!(common::directory_identity(root, &identity)?, current);
    let binding = IndependentTargetBinding {
        initialized: common::binding(&fixture.target.join("initialized.json"))?,
        registration: common::binding(&registration)?,
        initialized_files: common::binding(&fixture.local.join("initialized-files.json"))?,
        environment: registered["environment"].clone(),
        generation: json!({"path":original["path"],"manifest":original["manifest"],
            "storage":null,"cache":null}),
        state: original["state"].clone(),
    };
    let mut binding = TargetBinding::Independent(Box::new(binding));
    let copy = TargetBinding::Copy(document::read(&fixture.run.join("manifest.json"))?);
    let history = History::read(root, &binding, None)?;
    assert_eq!(
        history.initialized,
        History::read(root, &copy, None)?.initialized
    );
    let TargetBinding::Independent(bound) = &mut binding else {
        unreachable!();
    };
    let storage = Storage::acquire_independent_target(
        root,
        &current,
        &bound.registration,
        &bound.generation,
    )?;
    assert_eq!(
        bound.locked_storage(root, &storage.directory)?,
        storage.directory
    );
    assert!(bound.locked_storage(root, &current).is_err());
    let original_generation = bound.generation.clone();
    bound.generation["manifest"] = bound.registration.clone();
    assert!(bound.locked_storage(root, &storage.directory).is_err());
    bound.generation = original_generation;
    let original_state = common::binding(&storage.directory.join("state.json"))?;
    let substitute = current.join("state.json");
    fs::copy(storage.directory.join("state.json"), &substitute)?;
    bound.state = common::binding(&substitute)?;
    assert!(bound.locked_storage(root, &storage.directory).is_err());
    bound.state = original_state.clone();
    bound.state["sha256"] = json!("c".repeat(64));
    assert!(bound.locked_storage(root, &storage.directory).is_err());
    storage.finish(Ok(()))?;
    assert_eq!(
        common::binding(path(&original_state["path"], "原账本")?)?,
        original_state
    );
    bound.initialized_files = bound.registration.clone();
    assert!(History::read(root, &binding, None).is_err());
    Ok(())
}

#[test]
fn independent_fixture_requires_both_projected_services_from_the_same_arm_generation() -> Result<()>
{
    let initial = json!({"kind":"initial"});
    verify_fixture_generation(&initial, None, None)?;
    let restart = json!({"kind":"restart","attempt":4});
    let storage = json!({"generation":restart,"storage":{"identity":{"pid":1}}});
    let cache = json!({"generation":restart,"redis":{"pid":2}});
    let other_cache = json!({"generation":{"kind":"restart","attempt":5},"redis":{"pid":2}});
    verify_fixture_generation(&restart, Some(&storage), Some(&cache))?;
    assert!(verify_fixture_generation(&restart, Some(&storage), Some(&other_cache)).is_err());
    assert!(verify_fixture_generation(&restart, Some(&storage), None).is_err());
    assert!(verify_fixture_generation(&initial, Some(&storage), Some(&cache)).is_err());
    Ok(())
}

#[test]
fn reference_fence_checks_the_borrowed_authority_even_when_the_read_fails() {
    use std::cell::Cell;

    let checks = Cell::new(0);
    let result: Result<()> = reference_fence(
        || {
            let count = checks.get();
            checks.set(count + 1);
            if count == 0 {
                Ok(())
            } else {
                Err("目标存储账本变化".into())
            }
        },
        || Err("原观察失败".into()),
    );
    assert_eq!(checks.get(), 2);
    let error = result.unwrap_err().to_string();
    assert!(error.contains("原观察失败"));
    assert!(error.contains("目标存储账本变化"));
}

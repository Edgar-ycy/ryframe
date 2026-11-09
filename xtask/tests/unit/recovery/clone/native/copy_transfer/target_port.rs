use std::fs;

use super::*;

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

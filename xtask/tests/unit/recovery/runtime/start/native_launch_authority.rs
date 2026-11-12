use super::*;
use crate::recovery::runtime::dataset_authority::registration;

#[test]
fn ordinary_command_authority_rejects_retired_fields_versions_and_missing_tool() -> Result<()> {
    let fixture = configured()?;
    let verify = |request: &Value| {
        let mut inputs = Inputs::new(&fixture.backend);
        let result = commands::verify_direct(request, &mut inputs);
        inputs.finish(result)
    };
    assert_eq!(verify(&fixture.request)?, fixture.commands);

    for (field, value) in [
        ("variables", json!([])),
        ("sha256", json!("a".repeat(64))),
        ("extra", json!(true)),
    ] {
        let mut changed = fixture.request.clone();
        changed["environment"][field] = value;
        let error = verify(&changed).unwrap_err().to_string();
        assert!(
            error.contains("恢复环境绑定字段缺失或包含未知字段"),
            "{error}"
        );
    }
    for version in [json!(1), json!(true)] {
        let mut changed = fixture.request.clone();
        changed["format_version"] = version;
        let error = verify(&changed).unwrap_err().to_string();
        assert!(error.contains("恢复启动请求版本或类型不同"), "{error}");
    }
    let mut changed = fixture.request.clone();
    changed["tools"]
        .as_object_mut()
        .unwrap()
        .remove("frontend_server");
    let error = verify(&changed).unwrap_err().to_string();
    assert!(
        error.contains("恢复工具绑定字段缺失或包含未知字段"),
        "{error}"
    );
    assert!(!fixture.root.try_exists()?);
    assert!(!generation::creation_path(&fixture.root).try_exists()?);
    assert!(!fixture.control.join(REGISTRATION_LOCK).try_exists()?);

    let mut frontend = Command::new(std::env::current_exe()?);
    frontend
        .env("APP_ENV", "prod")
        .env("MYSQL_PWD", "synthetic-mysql-marker");
    actor::restrict_environment(&mut frontend, false);
    assert!(
        frontend
            .get_envs()
            .all(|(name, _)| name != "APP_ENV" && name != "MYSQL_PWD")
    );
    Ok(())
}

#[test]
fn ordinary_registration_rejects_boolean_version_and_remote_write_count() -> Result<()> {
    let fixture = configured()?;
    let path = fixture.control.join("registration.json");
    let original = strict::read(&path)?;
    let verify = || {
        let mut binding = fixture.binding.clone();
        binding["registration"] = file_binding(&fixture, &path)?;
        let mut inputs = Inputs::new(&fixture.backend);
        let registered = inputs.bound(&binding["registration"])?;
        let target = inputs.bound(&binding["target_plan"])?;
        let result = registration::read_binding(&mut inputs, &binding, &target, &registered);
        inputs.finish(result)
    };
    verify()?;
    for (field, value) in [
        ("format_version", json!(true)),
        ("remote_writes", json!(false)),
    ] {
        let mut changed = original.clone();
        changed[field] = value;
        strict::write_replace(&path, &changed)?;
        let error = verify().unwrap_err().to_string();
        assert!(
            error.contains("恢复运行登记类型、写入声明或目标绑定不同"),
            "{error}"
        );
    }
    assert!(!fixture.root.try_exists()?);
    Ok(())
}

fn configured() -> Result<Fixture> {
    let mut fixture = Fixture::new();
    fixture.root = fixture.control.join("case").join("runtime");
    let program = portable_path(&std::env::current_exe()?.canonicalize()?);
    drop(configure_with_artifact(
        &mut fixture,
        &program,
        &program,
        None,
    )?);
    Ok(fixture)
}

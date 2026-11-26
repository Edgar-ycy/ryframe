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
fn production_asset_cache_requires_the_bound_independent_b1_target() -> Result<()> {
    let fixture = configured()?;
    let tools = &fixture.request["tools"];
    let verify = |request: &Value| {
        let mut inputs = Inputs::new(&fixture.backend);
        let direct = commands::verify_direct(request, &mut inputs)?;
        let expected = commands::expected(request, tools, &mut inputs)?;
        assert_eq!(expected, direct);
        inputs.finish(Ok(direct))
    };
    assert_eq!(verify(&fixture.request)?, fixture.commands);

    let mut request = fixture.request.clone();
    request["unexpected"] = json!(true);
    assert!(verify(&request).is_err());
    request.as_object_mut().unwrap().remove("unexpected");
    for invalid in [json!(null), json!(true), json!("warm")] {
        request["frontend_asset_cache"] = invalid;
        let error = verify(&request).unwrap_err().to_string();
        assert!(error.contains("恢复前端资源缓存模式无效"), "{error}");
    }
    request["frontend_asset_cache"] = json!(commands::PRODUCTION_ASSET_CACHE_MODE);
    let error = verify(&request).unwrap_err().to_string();
    assert!(error.contains("仅适用于同一已登记独立 B1 目标"), "{error}");

    let (registered, target) = independent_cache_registration(&fixture)?;
    request["registration"] = json!({"registration":registered,"target_plan":target});
    let mut expected = fixture.commands.clone();
    expected["frontend"]
        .as_array_mut()
        .unwrap()
        .push(json!("--production-asset-cache"));
    assert_eq!(verify(&request)?, expected);

    let mut changed = request.clone();
    changed["authority"]["frontend_endpoint"] = json!("http://127.0.0.1:1");
    let error = verify(&changed).unwrap_err().to_string();
    assert!(error.contains("未绑定独立 B1 运行目录与端点"), "{error}");
    request["registration"]["target_plan"]["sha256"] = json!("d".repeat(64));
    assert!(verify(&request).is_err());
    assert!(!fixture.root.try_exists()?);
    Ok(())
}

fn independent_cache_registration(fixture: &Fixture) -> Result<(Value, Value)> {
    let reference_path = fixture.control.join("reference.json");
    let envelope = json!({
        "format_version":1,"kind":"restore-reference-independent-b1-plan",
        "reference_plan":strict::read(&reference_path)?,"arm_input":{},"source_export":{},
    });
    strict::write_replace(&reference_path, &envelope)?;
    let target_path = fixture.control.join("target.json");
    let mut target = strict::read(&target_path)?;
    target["kind"] = json!("restore-reference-target-plan");
    target["reference_plan_sha256"] = json!(document::hash(&envelope)?);
    strict::write_replace(&target_path, &target)?;
    let target_binding = file_binding(fixture, &target_path)?;
    let registration_path = fixture.control.join("registration.json");
    let mut registered = strict::read(&registration_path)?;
    registered["reference_plan"] = file_binding(fixture, &reference_path)?;
    registered["target_plan"] = target_binding.clone();
    strict::write_replace(&registration_path, &registered)?;
    Ok((file_binding(fixture, &registration_path)?, target_binding))
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

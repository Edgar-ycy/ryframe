use super::*;

#[test]
fn reference_dataset_schema_uses_bound_files_and_accepts_current_fixture_fields() {
    let fixture = Fixture::new();
    let backend = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let mut plan = fixture.plan.clone();
    let credentials = fixture.root.parent().unwrap().join("administrator.toml");
    fs::write(
        &credentials,
        "[administrator]\ntenant_id='system'\nusername='admin'\npassword='Current!Changed123'\n",
    )
    .unwrap();
    let credential_binding = json!({"path":credentials,"bytes":fs::metadata(&credentials).unwrap().len(),"sha256":hex::encode(Sha256::digest(fs::read(&credentials).unwrap()))});
    plan["dataset"] = json!({"administrator_credentials":credential_binding,"timeout_seconds":43200,"request_interval_ms":1000,"api_validation_posts":2,
        "post_batch_rows":1000,"tenant_targets":["control"],"records":100000,"object_count":256,"object_bytes":4194304,
        "admin":{"tenant_id":"system","username":"admin"},"configuration":plan["source"]["s3"]["configuration"]});
    plan["fixture_bindings"] = json!({"runtime":{"path":fixture.root.join("runtime.json")}});
    plan["source"]["frontend_url"] = json!("http://127.0.0.1:4174");
    inputs::domain::validate_plan(&plan, backend).unwrap();
    for (pointer, field) in [
        ("/dataset", "owner_password_env"),
        ("/dataset/admin", "password_env"),
        ("/dataset/admin", "password"),
        ("/dataset", "owner_password"),
    ] {
        let mut invalid = plan.clone();
        invalid.pointer_mut(pointer).unwrap()[field] = json!("private-do-not-echo");
        assert!(inputs::domain::validate_plan(&invalid, backend).is_err());
    }
    plan["dataset"]["configuration"]["sha256"] = json!("0".repeat(64));
    assert!(inputs::domain::validate_plan(&plan, backend).is_err());
}

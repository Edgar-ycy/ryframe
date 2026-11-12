use super::{configuration, files, fixture, source, write_receipt};
use crate::{
    Result,
    recovery::{clone::native::digest, fixture_control::portable_path},
};
use serde_json::{Value, json};
use std::{fs, path::Path};

const PRIVATE: &str = "synthetic-private-frontend-configuration-value";

#[test]
fn external_tool_override_binds_absolute_unicode_path_and_requires_selected_file() -> Result<()> {
    let temporary = fixture();
    let root = temporary.path();
    let external = tempfile::Builder::new()
        .prefix("外部 公开配置-")
        .tempdir_in(root.parent().unwrap())?;
    let directory = portable_path(&external.path().canonicalize()?);
    let selected = directory.join("app.test.toml");
    let mut application = fs::read(root.join("config/app.test.toml"))?;
    application.extend(b"# external selected configuration\n");
    fs::write(&selected, &application)?;
    let public_tools = root.join("config/tools.json");
    let local_tools = root.join(".local-tests/frontend-tools.json");
    fs::write(&public_tools, br#"{"application_config_dir":"config"}"#)?;
    fs::write(
        &local_tools,
        serde_json::to_vec(&json!({"application_config_dir":directory}))?,
    )?;

    let build = recorded_build(root)?;
    let head = build["identity"]["frontend_commit"].as_str().unwrap();
    let directory_name = directory.to_str().unwrap().replace('\\', "/");
    let expected = json!({
        "directory":directory_name,"environment":"test",
        "files":[{"path":format!("{directory_name}/app.test.toml"),"sha256":digest::file(&selected)?.sha256}],
        "tools":[
            {"path":"config/tools.json","sha256":digest::file(&public_tools)?.sha256},
            {"path":".local-tests/frontend-tools.json","sha256":digest::file(&local_tools)?.sha256},
        ],
    });
    assert!(directory.is_absolute() && !directory.starts_with(root));
    assert!(!directory.join("app.toml").try_exists()?);
    assert_eq!(
        configuration::verify(root, &build, Some(head))?.configuration,
        expected
    );

    fs::remove_file(&selected)?;
    reject_configuration(root, &build, Some(head), "必需文件或父目录不可读取");
    fs::create_dir(&selected)?;
    for expected in [None, Some(head)] {
        reject_configuration(root, &build, expected, "错误类型");
    }
    fs::remove_dir(&selected)?;
    fs::write(&selected, application)?;
    fs::write(
        &local_tools,
        serde_json::to_vec(&json!({"application_config_dir":directory.join("missing/child")}))?,
    )?;
    reject_configuration(root, &build, Some(head), "必需文件或父目录不可读取");
    Ok(())
}

#[test]
fn development_uses_explicit_environment_with_dotenv_present_and_no_managed_parents() -> Result<()>
{
    let temporary = fixture();
    let root = temporary.path();
    fs::remove_dir(root.join(".local-tests/build"))?;
    fs::remove_dir(root.join(".local-tests"))?;
    fs::write(
        root.join("dist/build-identity.json"),
        br#"{"frontend_commit":"development"}"#,
    )?;
    for name in [
        ".env",
        ".env.local",
        ".env.production",
        ".env.production.local",
    ] {
        fs::write(root.join(name), format!("VITE_APP_API_ORIGIN={PRIVATE}\n"))?;
    }
    let application = fs::read_to_string(root.join("config/app.test.toml"))?;
    for environment in ["dev", "test", "prod"] {
        let relative = format!("config/app.{environment}.toml");
        let selected = root.join(&relative);
        fs::write(&selected, format!("{application}\n# {environment}\n"))?;
        let inputs = configuration::capture(root, environment)?;
        assert_eq!(
            inputs.configuration,
            json!({"environment":environment,"directory":"config","tools":[],
                "files":[{"path":relative,"sha256":digest::file(&selected)?.sha256}]}),
        );
        assert_eq!(
            inputs.identity,
            json!({"frontend_commit":"development","source_file":null})
        );
        assert!(!root.join(".local-tests").try_exists()?);
    }
    Ok(())
}

#[test]
fn same_origin_and_server_host_preserve_distinct_ecmascript_whitespace_rules() -> Result<()> {
    let temporary = fixture();
    let root = temporary.path();
    let selected = root.join("config/app.test.toml");
    let application = |origin, host| {
        format!(
            "api_origin=\"{origin}\"\n[server]\nhost=\"{host}\"\nport=5173\nproxy_target=\"http://localhost:8080\"\n"
        )
    };
    fs::write(&selected, application(r"\uFEFF", r"\uFEFFlocalhost\uFEFF"))?;
    let build = recorded_build(root)?;
    configuration::verify(root, &build, None)?;
    for (origin, host, reason) in [
        ("https://example.invalid", "localhost", "api_origin 为空"),
        (r"\u0085", "localhost", "HTTP(S) origin 无效"),
        ("", r"\uFEFF", "server.host 必须非空"),
        ("", r"localhost\u0085", "server.host 必须非空"),
    ] {
        fs::write(&selected, application(origin, host))?;
        reject_configuration(root, &build, None, reason);
    }
    Ok(())
}

#[test]
fn tools_emit_only_bound_metadata_and_config_errors_hide_private_input() -> Result<()> {
    let temporary = fixture();
    let root = temporary.path();
    let tools = root.join("config/tools.json");
    let selected = root.join("config/app.test.toml");
    let original = serde_json::to_vec(&json!({
        "api_contract":{"private":PRIVATE},"real_browser":PRIVATE,"browser_smoke":[PRIVATE],
    }))?;
    fs::write(&tools, &original)?;
    let build = recorded_build(root)?;
    let inputs = configuration::verify(root, &build, None)?;
    assert_eq!(
        inputs.configuration["tools"],
        json!([{"path":"config/tools.json","sha256":digest::file(&tools)?.sha256}])
    );
    assert!(!inputs.configuration.to_string().contains(PRIVATE));
    drop(inputs);

    for (raw, reason) in [
        (
            format!(r#"{{"application_config_dir":["{PRIVATE}"]}}"#).into_bytes(),
            "字段必须是字符串",
        ),
        (
            format!(r#"["{PRIVATE}"]"#).into_bytes(),
            "JSON 文档必须是对象",
        ),
        (
            format!(r#"{{"application_config_dir":NaN,"real_browser":"{PRIVATE}"}}"#).into_bytes(),
            "JSON 语法、重复键或数值无效",
        ),
        (
            [PRIVATE.as_bytes(), &[0xff]].concat(),
            "JSON 文档 UTF8 无效",
        ),
    ] {
        fs::write(&tools, raw)?;
        reject_configuration(root, &build, None, reason);
    }
    fs::write(&tools, original)?;
    for (raw, reason) in [
        (
            format!("api_origin=[\"{PRIVATE}\"]\n"),
            "origin 必须是字符串",
        ),
        (
            format!("server=[\"{PRIVATE}\"]\n"),
            "TOML 字段必须是公开 table",
        ),
        (
            format!("# {PRIVATE}\n[server]\nport=nan\n"),
            "server.port 必须为",
        ),
        (
            format!("# {PRIVATE}\napi_origin=\"\"\n"),
            "缺少 api_origin 或 server.host",
        ),
    ] {
        fs::write(&selected, raw)?;
        reject_configuration(root, &build, None, reason);
    }
    Ok(())
}

fn recorded_build(root: &Path) -> Result<Value> {
    let inventory = source::inventory_value(&source::capture(root)?);
    let head = inventory["source"]["snapshot"]["head"].as_str().unwrap();
    write_receipt(root, head, &inventory);
    let receipt = files::parse(&fs::read(root.join("dist").join(files::RECEIPT))?)?;
    Ok(receipt["build"].clone())
}

fn reject_configuration(root: &Path, build: &Value, head: Option<&str>, reason: &str) {
    let error = configuration::verify(root, build, head)
        .err()
        .expect("无效前端配置必须拒绝")
        .to_string();
    assert!(!error.contains(PRIVATE), "诊断回显了私有配置值");
    assert!(error.contains(reason), "预期诊断 {reason}，实际 {error}");
}

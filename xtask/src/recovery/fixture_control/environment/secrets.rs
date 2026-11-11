//! 只生成或复制隔离秘密文件，不启动服务或写入远端资源。
use super::super::write_new;
use super::protocol::GuardError;
use crate::{
    Result,
    cli::{FixtureEnvironmentCommand, FixtureSecretCredential},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use std::{fs, path::Path};
#[path = "secrets/facts.rs"]
mod facts;
#[path = "secrets/files.rs"]
mod files;

pub(crate) fn run(root: &Path, command: &FixtureEnvironmentCommand) -> Result<Value> {
    run_with(root, command, |_| Ok(()))
}

/// 文件同步后的局部观察缝隙；正式入口固定使用无副作用回调。
pub(crate) fn run_with(
    root: &Path,
    command: &FixtureEnvironmentCommand,
    mut after_sync: impl FnMut(&Path) -> Result<()>,
) -> Result<Value> {
    match command {
        FixtureEnvironmentCommand::RotateSecrets {
            fixture,
            output,
            credential,
            secrets_dir,
        } => rotate(
            root,
            fixture,
            output,
            *credential,
            secrets_dir.as_deref(),
            &mut after_sync,
        )?,
        FixtureEnvironmentCommand::BootstrapSecrets {
            source_fixture,
            source_secrets,
            fixture,
        } => bootstrap(
            root,
            source_fixture,
            source_secrets,
            fixture,
            &mut after_sync,
        )?,
        _ => return Err(Box::new(GuardError("不是原生秘密文件操作"))),
    };
    Ok(
        json!({"status":if matches!(command, FixtureEnvironmentCommand::RotateSecrets { .. }) {
        "generated" } else { "imported" },"services_started":false,"remote_writes":0}),
    )
}

fn rotate(
    root: &Path,
    fixture: &Path,
    output: &Path,
    credential: FixtureSecretCredential,
    secrets_dir: Option<&Path>,
    after_sync: &mut impl FnMut(&Path) -> Result<()>,
) -> Result<()> {
    let fixture = facts::Fixture::open(root, fixture)?;
    let source = files::Frozen::open(root, &fixture.execution, secrets_dir)?;
    facts::output(root, &fixture.execution, output)?;
    let mut contents = source.contents.clone();
    let mut random = [0_u8; 32];
    rustls::crypto::aws_lc_rs::default_provider()
        .secure_random
        .fill(&mut random)
        .map_err(|_| "安全密码随机数生成失败，未创建输出目录")?;
    let encoded = URL_SAFE_NO_PAD.encode(random);
    let (name, value) = match credential {
        FixtureSecretCredential::Administrator => {
            let password = format!("Aa1!{encoded}");
            super::guards::reset_password(&json!({"admin_password":password}))?;
            ("reset-admin-password.txt", password)
        }
        FixtureSecretCredential::Jwt => {
            let name = "jwt-secret.txt";
            if std::str::from_utf8(&contents[name]).is_ok_and(|source| source.trim() == encoded) {
                return Err("生成的 JWT 密钥与源密钥相同，未创建输出目录".into());
            }
            (name, encoded)
        }
    };
    contents.insert(name.into(), format!("{value}\n").into_bytes());
    fs::create_dir(output)?;
    let mut stage = "复制秘密文件";
    let result = (|| {
        files::publish(output, &contents, after_sync)?;
        stage = "复核秘密源";
        source.check()?;
        stage = "复核夹具与源码快照";
        fixture.check()?;
        stage = "复核目标字节";
        let bindings = files::verify(output, &contents)?;
        stage = "同步成功收据";
        write_new(
            &output.join("secret-set.json"),
            &json!({"format_version":1,
            "kind":"reference-fixture-secret-set","status":"generated",
            "source":source.bindings,"files":bindings}),
        )
    })();
    finish(output, false, stage, result)
}

fn bootstrap(
    root: &Path,
    source_fixture: &Path,
    selected: &Path,
    fixture: &Path,
    after_sync: &mut impl FnMut(&Path) -> Result<()>,
) -> Result<()> {
    let source_fixture = facts::Fixture::open(root, source_fixture)?;
    let target = facts::Fixture::open(root, fixture)?;
    if source_fixture.execution == target.execution {
        return Err(Box::new(GuardError("秘密导入必须使用两个不同执行工作树")));
    }
    let source = files::Frozen::open(root, &source_fixture.execution, Some(selected))?;
    let output = target
        .execution
        .join(".local-tests/reference-fixture/secrets");
    facts::output(root, &target.execution, &output)?;
    fs::create_dir(&output)?;
    let mut stage = "复制秘密文件";
    let result = (|| {
        files::publish(&output, &source.contents, after_sync)?;
        stage = "复核秘密源";
        source.check()?;
        stage = "复核两侧夹具与源码快照";
        source_fixture.check()?;
        target.check()?;
        stage = "复核目标字节";
        let bindings = files::verify(&output, &source.contents)?;
        stage = "同步成功收据";
        write_new(
            &output.join("bootstrap.json"),
            &json!({"format_version":1,
            "kind":"reference-fixture-secret-bootstrap","status":"imported",
            "source_fixture":source_fixture.binding,"source_files":source.bindings,
            "target_fixture":target.binding,"target_files":bindings,
            "remote_writes":0,"services_started":false}),
        )
    })();
    finish(&output, true, stage, result)
}

fn finish(output: &Path, bootstrap: bool, stage: &str, result: Result<()>) -> Result<()> {
    let Err(error) = result else {
        return Ok(());
    };
    let reason = safe_reason(error.as_ref());
    let failed = if bootstrap {
        json!({"status":"failed","remote_writes":0,"services_started":false})
    } else {
        json!({"status":"failed"})
    };
    let marker = write_new(&output.join("failed.json"), &failed);
    let marker_status = match marker {
        Ok(()) => "已同步".to_owned(),
        Err(error) => format!(
            "未能落盘（{}），目录仍不得消费",
            safe_reason(error.as_ref())
        ),
    };
    Err(format!("秘密文件阶段“{stage}”失败：{reason}；本地目录已写入并保留：{}；失败标记{marker_status}；无远端写入或服务启动。请核对源文件绑定、两侧夹具/G快照、目标前后像及文件权限/磁盘状态，使用新的隔离目录，不自动重试或覆盖。",
        output.display()).into())
}

fn safe_reason(error: &(dyn std::error::Error + 'static)) -> String {
    let mut current = Some(error);
    for _ in 0..16 {
        let Some(error) = current else {
            break;
        };
        if let Some(error) = error.downcast_ref::<GuardError>() {
            return error.0.to_owned();
        }
        if let Some(error) = error.downcast_ref::<std::io::Error>() {
            return format!(
                "I/O 操作失败（kind={:?}，raw_os_error={:?}）",
                error.kind(),
                error.raw_os_error()
            );
        }
        current = error.source();
    }
    "上游核验未完成，细因待检查".to_owned()
}

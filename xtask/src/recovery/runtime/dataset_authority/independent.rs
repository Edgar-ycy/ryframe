//! 从已完成的独立 B1 恢复读取运行权威，读取窗口由调用方持有。
use std::path::Path;

use serde_json::Value;

use super::{check, configuration_registration, inputs::Inputs};
use crate::{
    Result,
    recovery::inputs::{
        backup::Backup,
        bindings::{verify_advance, with_completed_target},
        comparison,
        document::{self, Document},
        target::TargetRequest,
    },
};

pub(in crate::recovery::runtime) struct TargetProjection {
    pub(in crate::recovery::runtime) reference: Value,
    pub(in crate::recovery::runtime) target: Value,
    pub(in crate::recovery::runtime) backup: Backup,
    pub(in crate::recovery::runtime) backend_environment: Value,
}

pub(in crate::recovery::runtime) fn read(
    backend: &Path,
    reference: &Document,
    target: &Document,
    expected_registration: &Value,
    bindings: &Value,
    inputs: &mut Inputs,
) -> Result<TargetProjection> {
    let backup = inputs.bound(&target.value["backup_receipt"])?;
    with_completed_target(
        backend,
        reference,
        target,
        &backup,
        |evidence, completed| {
            let manifest = &completed.manifest().value;
            verify_handoff(
                completed.registration(),
                expected_registration,
                &completed.running().value,
                manifest,
                bindings,
            )?;
            verify_registered_configuration(
                inputs,
                completed.registration(),
                &evidence.arm.target_request,
            )?;
            let arm = comparison::arm(&evidence.comparison_document.value, "candidate")?;
            Ok(TargetProjection {
                reference: evidence.reference_plan().clone(),
                target: target.value.clone(),
                backup: Backup {
                    manifest: manifest.clone(),
                    result: evidence.backup.result.clone(),
                },
                backend_environment: arm["backend_environment"].clone(),
            })
        },
    )
}

fn verify_registered_configuration(
    inputs: &mut Inputs,
    registration: &Document,
    target: &TargetRequest,
) -> Result<()> {
    let environment = configuration_registration::read(inputs, registration)?;
    configuration_registration::Source::from_target(target)?.verify(&environment)
}

fn verify_handoff(
    registration: &Document,
    expected_registration: &Value,
    running: &Value,
    manifest: &Value,
    bindings: &Value,
) -> Result<()> {
    document::exact(bindings, &["record", "manifest"], "独立 B1 恢复 bindings")?;
    check(
        document::binding(registration) == *expected_registration,
        "独立 B1 运行未绑定已完成恢复收据中的原运行登记",
    )?;
    check(
        bindings["manifest"] == *manifest,
        "独立 B1 运行数据绑定未使用已完成恢复的原备份清单",
    )?;
    verify_advance(running, &bindings["record"], manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery::{
        clone::native::target_verify::input::request_configuration::{
            canonical_path, configuration_files, configuration_identity,
        },
        runtime::dataset_authority::reads::Window,
    };
    use serde_json::json;
    use std::{collections::BTreeMap, fs};

    #[test]
    fn independent_runtime_rejects_role_configuration_drift_under_the_original_registration()
    -> Result<()> {
        let backend = crate::workspace::root_dir();
        let local = backend.join(".local-tests");
        fs::create_dir_all(&local)?;
        let temporary = tempfile::tempdir_in(&local)?;
        let execution = canonical_path(temporary.path())?;
        let configuration = execution.join("fresh-config");
        fs::create_dir(&configuration)?;
        fs::write(
            configuration.join("app.test.toml"),
            "scope_id = 'independent-b1'\nsnowflake_worker_id = 7\n[jobs]\nmode = 'external'\n[monitor]\nmetrics_bearer_token = 'fixture-token'\n[logger]\nlevel = 'info'\n",
        )?;
        let (directory, files) = configuration_files(
            &execution,
            &BTreeMap::from([("APP_ENV".into(), "test".into())]),
            Some(&json!(configuration)),
        )?;
        let target = TargetRequest {
            document: Document {
                path: execution.join("fresh-request.json"),
                raw: Vec::new(),
                value: json!({
                    "config_dir": directory,
                    "configuration_sha256": configuration_identity(&directory, &files)?,
                }),
            },
            selected: Value::Null,
            execution: execution.clone(),
            execution_binding: Value::Null,
        };
        let registration_path = execution.join("runtime-registration.json");
        let environment = configuration_registration::Source::from_target(&target)?
            .publish(&backend, &registration_path)?;
        fs::write(
            &registration_path,
            document::canonical_bytes(&json!({"runtime_environment":environment}))?,
        )?;
        let registration = document::read(&registration_path, &backend, "测试运行登记")?;
        let verify = || -> Result<()> {
            let window = Window::direct()?;
            let mut inputs = Inputs::new(&backend);
            let result = verify_registered_configuration(&mut inputs, &registration, &target);
            window.finish(inputs.finish(result))
        };
        verify()?;

        let worker = configuration_registration::sidecar(&registration_path)?
            .join("roles/worker-config/app.test.toml");
        let mut changed: toml::Table = toml::from_str(&fs::read_to_string(&worker)?)?;
        changed
            .get_mut("logger")
            .and_then(toml::Value::as_table_mut)
            .ok_or("测试角色缺少日志配置")?
            .insert("level".into(), toml::Value::String("warn".into()));
        fs::write(worker, toml::to_string(&changed)?)?;
        let error = verify().unwrap_err();
        assert!(error.to_string().contains("角色配置的文件字节已变化"));
        registration.assert_unchanged()
    }

    #[test]
    fn independent_runtime_handoff_rejects_replaced_registration_manifest_and_record() {
        let registration = Document {
            path: "runtime-registration.json".into(),
            raw: b"{}".to_vec(),
            value: json!({}),
        };
        let expected = document::binding(&registration);
        let running = json!({
            "plan":{"id":"independent-b1"},"plan_hash":"a".repeat(64),"status":"running",
            "started_at":"2026-11-11T04:00:00.123456Z",
            "recovered_at":"2026-11-11T03:57:15.008900Z",
            "data_verified_at":null,"completed_at":null,"failure":null
        });
        let mut record = running.clone();
        record["status"] = json!("data_verified");
        record["data_verified_at"] = json!("2026-11-11T05:00:00.123456Z");
        let manifest = json!({"id":"shared", "captured_at":running["recovered_at"]});
        let bindings = json!({"record":record,"manifest":manifest});
        verify_handoff(&registration, &expected, &running, &manifest, &bindings).unwrap();

        let moved = Document {
            path: "other/runtime-registration.json".into(),
            ..registration.clone()
        };
        assert!(verify_handoff(&moved, &expected, &running, &manifest, &bindings).is_err());
        for (path, replacement) in [
            ("/manifest/id", json!("other-backup")),
            ("/record/started_at", json!("2026-11-11T04:00:00.123457Z")),
        ] {
            let mut replaced = bindings.clone();
            *replaced.pointer_mut(path).unwrap() = replacement;
            assert!(
                verify_handoff(&registration, &expected, &running, &manifest, &replaced).is_err()
            );
        }
    }
}

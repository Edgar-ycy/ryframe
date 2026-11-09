//! DevEx 性能验证只读观察同一正式恢复运行代次。
use std::path::Path;

use serde_json::{Value, json};

use super::{
    bind::control::{Control, Operation},
    dataset_authority::{check, commands, cross, inputs::Inputs, path, reads::Window, schema},
    verify,
};
use crate::{
    Result,
    cli::RuntimeSources,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::{
        clone::native::json as strict,
        inputs::document::{self, Document},
    },
};

pub(crate) fn observe(
    driver: &Path,
    backend: &Path,
    frontend: &Path,
    receipts: &Value,
) -> Result<Value> {
    let window = Window::direct()?;
    let mut inputs = Inputs::new(driver);
    let result = (|| {
        validate_absolute_path(driver, LocalTestPathKind::ExistingDirectory)?;
        let explicit = Explicit::read(&mut inputs, receipts)?;
        let before = explicit.control(driver, &mut inputs)?;
        let observed = explicit.online(driver, backend, frontend, &before, &mut inputs);
        let after = explicit.control(driver, &mut inputs).and_then(|current| {
            check(
                current.snapshot == before.snapshot && current.operation == before.operation,
                "性能核验期间恢复运行生命周期或代次发生变化",
            )
        });
        let observed = finish(observed, after)?;
        Ok(json!({
            "generation":before.launch.value["generation"],
            "lifecycle":before.snapshot["lifecycle"],
            "verification":observed.reply(&explicit.runtime),
        }))
    })();
    window.finish(inputs.finish(result))
}

struct Explicit {
    runtime: Document,
    target: Document,
    registration: Document,
    launch: Document,
}

impl Explicit {
    fn read(inputs: &mut Inputs, receipts: &Value) -> Result<Self> {
        strict::exact(
            receipts,
            &[
                "runtime_receipt",
                "target_plan",
                "runtime_registration",
                "runtime_launch",
            ],
            "性能恢复运行收据绑定",
        )?;
        let explicit = Self {
            runtime: inputs.bound(&receipts["runtime_receipt"])?,
            target: inputs.bound(&receipts["target_plan"])?,
            registration: inputs.bound(&receipts["runtime_registration"])?,
            launch: inputs.bound(&receipts["runtime_launch"])?,
        };
        schema::runtime(&explicit.runtime.value)?;
        Ok(explicit)
    }

    fn control(&self, driver: &Path, inputs: &mut Inputs) -> Result<Control> {
        let control = Control::read(inputs, &self.launch.path, Operation::Verify)?;
        self.check_explicit(&control)?;
        let runtime = &self.runtime.value;
        let request = &control.launch.value["request"];
        check(
            commands::same_path(driver, &path(&request["tools"]["coordinator_root"])?)?
                && commands::same_path(
                    &path(&runtime["paths"]["runtime_dir"])?,
                    &path(&control.launch.value["runtime_directory"])?,
                )?
                && commands::same_path(&path(&runtime["paths"]["launch"])?, &self.launch.path)?
                && runtime["digests"]["launch"] == self.launch.sha256(),
            "性能恢复运行收据的协调根、当代目录或 launch 原始摘要不同",
        )?;
        Ok(control)
    }

    fn check_explicit(&self, control: &Control) -> Result<()> {
        let registration = document::binding(&self.registration);
        let target = document::binding(&self.target);
        check(
            control.snapshot["binding"]
                == json!({"registration":registration,"target_plan":target})
                && control.snapshot["registration"] == registration
                && control.snapshot["target_plan"] == target
                && control.snapshot["launch"] == document::binding(&self.launch)
                && control.launch.path == self.launch.path
                && control.launch.raw == self.launch.raw,
            "性能显式登记、目标计划或启动收据与原代次的路径或原始字节不同",
        )
    }

    fn online(
        &self,
        driver: &Path,
        backend: &Path,
        frontend: &Path,
        control: &Control,
        inputs: &mut Inputs,
    ) -> Result<verify::Observed> {
        let runtime = &self.runtime.value;
        let reference = inputs.bound(&control.snapshot["reference_plan"])?;
        let documents = cross::read(inputs, runtime)?;
        let authority = cross::verify(
            runtime,
            &self.target.value,
            &reference.value,
            &documents.bindings.value["manifest"],
            &documents,
        )?;
        let adapter = &authority["backend_adapter_contract"];
        let (adapter_contract, product_backend) = if adapter.is_null() {
            (None, None)
        } else {
            (
                Some(strict::text(adapter, "性能恢复后端适配合同")?.to_owned()),
                Some(path(&runtime["paths"]["backend_product_root"])?),
            )
        };
        let sources = RuntimeSources {
            source_backend: backend.to_owned(),
            source_frontend: frontend.to_owned(),
            adapter_contract,
            product_backend,
        };
        super::validate_sources(&sources)?;
        verify::online(
            driver,
            &sources,
            &documents.bindings.path,
            runtime,
            &authority,
            inputs,
        )
    }
}

fn finish<T>(result: Result<T>, after: Result<()>) -> Result<T> {
    match (result, after) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(after)) => {
            Err(format!("{error}；性能恢复运行控制后验同时失败：{after}").into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_runtime_control_requires_each_original_binding_and_launch_bytes() -> Result<()> {
        let document = |name: &str| Document {
            path: name.into(),
            raw: b"{}".to_vec(),
            value: json!({}),
        };
        let explicit = Explicit {
            runtime: document("runtime.json"),
            target: document("target.json"),
            registration: document("registration.json"),
            launch: document("runtime-launch.json"),
        };
        let registration = document::binding(&explicit.registration);
        let target = document::binding(&explicit.target);
        let mut control = Control {
            launch: explicit.launch.clone(),
            directory: "generation-0001".into(),
            operation: "runtime-verify".into(),
            snapshot: json!({
                "binding":{"registration":registration,"target_plan":target},
                "registration":registration,"target_plan":target,"launch":document::binding(&explicit.launch),
            }),
        };
        explicit.check_explicit(&control)?;
        for field in ["binding", "registration", "target_plan", "launch"] {
            let original = std::mem::replace(&mut control.snapshot[field], Value::Null);
            assert!(explicit.check_explicit(&control).is_err(), "{field}");
            control.snapshot[field] = original;
        }
        control.launch.raw = b"{}\n".to_vec();
        assert!(explicit.check_explicit(&control).is_err());
        Ok(())
    }

    #[test]
    fn performance_failure_preserves_the_runtime_control_postcheck_failure() {
        let error = finish::<()>(Err("在线探针失败".into()), Err("原运行代次变化".into()))
            .unwrap_err()
            .to_string();
        assert!(error.contains("在线探针失败"));
        assert!(error.contains("原运行代次变化"));
    }
}

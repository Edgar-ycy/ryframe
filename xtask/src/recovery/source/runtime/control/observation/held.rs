//! Windows 源启动采集持续持有已验证构建，运行角色和物理资源仍逐次观察。
use super::Observation;
use crate::{
    Result,
    recovery::{
        clone::native::{
            export::transport::Transport, json as document, resources::common,
            target_verify::guard::Inputs,
        },
        fixture_control::artifact,
        source::evidence::running::{builds::held::HeldBuilds, roles},
    },
};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

pub(super) fn capture<T: Transport + Sync>(
    observation: &Observation<'_, '_, T>,
    phase: &str,
) -> Result<(Value, Value)> {
    let expected = observation.full_checkpoint()?;
    let before = phase == "before";
    let opened = (|| {
        if before {
            observation.predecessor_quiet()?;
        }
        HeldStart::open(observation, before, &expected)
    })();
    let held = match opened {
        Ok(held) => held,
        Err(error) => return combine(Err(error), finish_boundary(observation, before)),
    };
    let checkpoint = || held.checkpoint(observation);
    let result = checkpoint().and_then(|()| observation.capture_with(phase, &checkpoint));
    let result = combine(result, checkpoint());
    let result = combine(result, held.inputs.check());
    let result = combine(
        result,
        held.builds.finish(
            observation.frame.backend,
            &observation.prepared.facts["request"],
        ),
    );
    let result = combine(result, finish_boundary(observation, before));
    combine(result, checkpoint())
}

fn finish_boundary<T: Transport + Sync>(
    observation: &Observation<'_, '_, T>,
    before: bool,
) -> Result<()> {
    let quiet = if before {
        observation.predecessor_quiet()
    } else {
        Ok(())
    };
    combine(quiet, observation.full_checkpoint().map(drop))
}

struct HeldStart {
    builds: HeldBuilds,
    inputs: Inputs,
    runtime: PathBuf,
    roles: RoleCheck,
}

impl HeldStart {
    fn open<T: Transport + Sync>(
        observation: &Observation<'_, '_, T>,
        before: bool,
        expected: &Value,
    ) -> Result<Self> {
        let output = observation.output()?;
        let runtime = output.join("runtime");
        let request = output.join("request.json");
        let intent = output.join("intent.json");
        let inputs = Inputs::open([
            request.clone(),
            intent.clone(),
            runtime.join("binaries.json"),
            runtime.join("runtime.json"),
        ])?;
        let value = document::read(&intent)?;
        document::exact(
            &value,
            &[
                "format_version",
                "kind",
                "request",
                "runtime_directory",
                "operations",
            ],
            "源启动固定意图",
        )?;
        if value["format_version"] != 1
            || value["kind"] != "seed-source-runtime-intent"
            || value["request"] != common::binding(&request)?
            || value["runtime_directory"] != json!(runtime)
        {
            return Err("持有源启动意图与本次请求、代次或运行目录不同".into());
        }
        roles::directory(&runtime, &value["operations"])?;
        let roles = if before {
            RoleCheck::Before
        } else {
            RoleCheck::running(&runtime, value)?
        };
        let builds = HeldBuilds::open(
            observation.frame.backend,
            &observation.prepared.facts["request"],
            expected,
        )?;
        inputs.check()?;
        Ok(Self {
            builds,
            inputs,
            runtime,
            roles,
        })
    }

    fn checkpoint<T: Transport + Sync>(&self, observation: &Observation<'_, '_, T>) -> Result<()> {
        self.inputs.check_identities()?;
        require_generation_runtime(&self.runtime)?;
        observation.checkpoint_with(
            || observation.frame.checkpoint(),
            |backend, request| self.builds.checkpoint(backend, request),
            |execution, runtime| {
                Ok(artifact::runtime::verify_configuration(execution, runtime)?.receipt)
            },
        )?;
        let facts = &observation.prepared.facts;
        self.roles.check(
            observation.frame.backend,
            &self.runtime,
            &facts["selected"],
            &observation.contract,
        )?;
        require_generation_runtime(&self.runtime)?;
        self.inputs.check_identities()
    }
}

fn require_generation_runtime(runtime: &Path) -> Result<()> {
    for name in ["source-pair.json", "runtime-evidence.json"] {
        match std::fs::symlink_metadata(runtime.join(name)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
            Ok(_) => return Err("源代次运行目录出现意外的全栈构建来源证据".into()),
        }
    }
    Ok(())
}

enum RoleCheck {
    Before,
    Running { intent: Value, evidence: Value },
}

impl RoleCheck {
    fn running(runtime: &Path, intent: Value) -> Result<Self> {
        let mut evidence = Map::new();
        for role in ["api", "worker"] {
            evidence.insert(
                role.into(),
                json!({
                    "process":common::binding(&runtime.join(format!("{role}.json")))?,
                    "tree":common::binding(&runtime.join(format!("{role}-tree.json")))?,
                    "ready":true,
                }),
            );
        }
        Ok(Self::Running {
            intent,
            evidence: json!({"roles":evidence}),
        })
    }

    fn check(
        &self,
        backend: &Path,
        runtime: &Path,
        selected: &Value,
        contract: &Value,
    ) -> Result<()> {
        match self {
            Self::Before => common::require_closed_runtime_ports(
                document::text(&selected["api_url"], "源 API")?,
                document::text(&contract["worker_ready_url"], "源 Worker ready")?,
            ),
            Self::Running { intent, evidence } => {
                roles::observe(runtime, evidence, intent, selected, contract, true)?;
                for role in ["api", "worker"] {
                    for field in ["process", "tree"] {
                        common::bound_file(
                            backend,
                            &evidence["roles"][role][field],
                            "持有源启动的原角色收据",
                        )?;
                    }
                }
                Ok(())
            }
        }
    }
}

fn combine<T>(result: Result<T>, checked: Result<()>) -> Result<T> {
    match (result, checked) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(check)) => {
            Err(format!("{error}。源启动观察收口复核同时失败：{check}").into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, net::TcpListener};

    #[test]
    fn held_start_checks_both_ports_again_after_ownership_changes() {
        let api = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let worker = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let selected = json!({"api_url":format!("http://{}", api.local_addr().unwrap())});
        let contract =
            json!({"worker_ready_url":format!("http://{}/readyz", worker.local_addr().unwrap())});
        let before = RoleCheck::Before;
        let path = Path::new(".");
        let check = || before.check(path, path, &selected, &contract);
        assert!(check().is_err());
        let api_addr = api.local_addr().unwrap();
        drop(api);
        assert!(check().is_err());
        drop(worker);
        check().unwrap();
        let successor = TcpListener::bind(api_addr).unwrap();
        assert!(check().is_err());
        drop(successor);
        check().unwrap();
    }

    #[test]
    fn held_start_rejects_new_full_stack_source_evidence() {
        let local = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        let runtime = tempfile::tempdir_in(local).unwrap();
        require_generation_runtime(runtime.path()).unwrap();
        for name in ["source-pair.json", "runtime-evidence.json"] {
            let path = runtime.path().join(name);
            fs::write(&path, b"{}").unwrap();
            assert!(require_generation_runtime(runtime.path()).is_err());
            fs::remove_file(path).unwrap();
        }
        require_generation_runtime(runtime.path()).unwrap();
    }
}

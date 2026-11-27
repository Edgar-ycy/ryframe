//! Windows 来源观察持续持有已验证构建，运行角色和物理资源仍逐次观察。
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    Before,
    Running,
    AfterPrelaunch,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Self::Before => "before",
            Self::Running => "running",
            Self::AfterPrelaunch => "after",
        }
    }

    fn requires_quiet(self) -> bool {
        self != Self::Running
    }
}

pub(super) fn capture<T: Transport + Sync>(
    observation: &Observation<'_, '_, T>,
    phase: Phase,
) -> Result<(Value, Value)> {
    let expected = observation.full_checkpoint()?;
    let opened = (|| {
        if phase.requires_quiet() {
            observation.predecessor_quiet()?;
        }
        HeldObservation::open(observation, phase, &expected)
    })();
    let held = match opened {
        Ok(held) => held,
        Err(error) => return combine(Err(error), finish_boundary(observation, phase)),
    };
    let checkpoint = || held.checkpoint(observation);
    let result = checkpoint().and_then(|()| observation.capture_with(phase.name(), &checkpoint));
    let result = combine(result, checkpoint());
    let result = combine(result, held.inputs.check());
    let result = combine(
        result,
        held.builds.finish(
            observation.frame.backend,
            &observation.prepared.facts["request"],
        ),
    );
    let result = combine(result, finish_boundary(observation, phase));
    combine(result, checkpoint())
}

fn finish_boundary<T: Transport + Sync>(
    observation: &Observation<'_, '_, T>,
    phase: Phase,
) -> Result<()> {
    let quiet = if phase.requires_quiet() {
        observation.predecessor_quiet()
    } else {
        Ok(())
    };
    combine(quiet, observation.full_checkpoint().map(drop))
}

struct HeldObservation {
    builds: HeldBuilds,
    inputs: Inputs,
    runtime: PathBuf,
    roles: RoleCheck,
    phase: Phase,
}

impl HeldObservation {
    fn open<T: Transport + Sync>(
        observation: &Observation<'_, '_, T>,
        phase: Phase,
        expected: &Value,
    ) -> Result<Self> {
        let output = observation.output()?;
        let runtime = output.join("runtime");
        let (inputs, roles) = role_inputs(output, &runtime, phase)?;
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
            phase,
        })
    }

    fn checkpoint<T: Transport + Sync>(&self, observation: &Observation<'_, '_, T>) -> Result<()> {
        self.inputs.check_identities()?;
        self.check_paths(observation.output()?)?;
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
        self.check_paths(observation.output()?)?;
        self.inputs.check_identities()
    }

    fn check_paths(&self, output: &Path) -> Result<()> {
        if self.phase == Phase::AfterPrelaunch {
            require_prelaunch_output(output)?;
        }
        require_generation_runtime(&self.runtime)
    }
}

fn role_inputs(output: &Path, runtime: &Path, phase: Phase) -> Result<(Inputs, RoleCheck)> {
    let [binaries, contract] = [runtime.join("binaries.json"), runtime.join("runtime.json")];
    if phase == Phase::AfterPrelaunch {
        require_prelaunch_output(output)?;
        return Ok((Inputs::open([binaries, contract])?, RoleCheck::Closed));
    }
    let request = output.join("request.json");
    let intent = output.join("intent.json");
    let inputs = Inputs::open([request.clone(), intent.clone(), binaries, contract])?;
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
    roles::directory(runtime, &value["operations"])?;
    let roles = if phase == Phase::Running {
        RoleCheck::running(runtime, value)?
    } else {
        RoleCheck::Closed
    };
    Ok((inputs, roles))
}

fn require_prelaunch_output(output: &Path) -> Result<()> {
    require_absent(
        output,
        &["request.json", "intent.json"],
        "未启动恢复观察目录出现启动请求或意图",
    )
}

fn require_generation_runtime(runtime: &Path) -> Result<()> {
    require_absent(
        runtime,
        &["source-pair.json", "runtime-evidence.json"],
        "源代次运行目录出现意外的全栈构建来源证据",
    )
}

fn require_absent(directory: &Path, names: &[&str], message: &str) -> Result<()> {
    for name in names {
        match std::fs::symlink_metadata(directory.join(name)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
            Ok(_) => return Err(message.to_owned().into()),
        }
    }
    Ok(())
}

enum RoleCheck {
    Closed,
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
            Self::Closed => common::require_closed_runtime_ports(
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
            Err(format!("{error}。源代次观察收口复核同时失败：{check}").into())
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
        let before = RoleCheck::Closed;
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

    #[test]
    fn held_prelaunch_after_requires_absent_start_records() {
        let local = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        let output = tempfile::tempdir_in(local).unwrap();
        let runtime = output.path().join("runtime");
        fs::create_dir(&runtime).unwrap();
        for name in ["binaries.json", "runtime.json"] {
            fs::write(runtime.join(name), b"{}").unwrap();
        }
        let (inputs, roles) = role_inputs(output.path(), &runtime, Phase::AfterPrelaunch).unwrap();
        assert!(matches!(roles, RoleCheck::Closed));
        inputs.check().unwrap();
        for name in ["request.json", "intent.json"] {
            let unexpected = output.path().join(name);
            fs::write(&unexpected, b"{}").unwrap();
            assert!(role_inputs(output.path(), &runtime, Phase::AfterPrelaunch).is_err());
            fs::remove_file(unexpected).unwrap();
        }
        require_prelaunch_output(output.path()).unwrap();
    }
}

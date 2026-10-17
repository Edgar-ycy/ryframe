//! 供应链策略、固定引用、本地 patch 许可证与 Cargo 依赖图的纯 Rust 只读门禁。

use std::path::Path;

use crate::{Result, process::PreservedFailure};

#[path = "supply_chain/cargo_graph.rs"]
pub(crate) mod cargo_graph;
#[path = "supply_chain/local_patch.rs"]
pub(crate) mod local_patch;
#[path = "supply_chain/policy.rs"]
pub(crate) mod policy;
#[path = "supply_chain/workflow.rs"]
pub(crate) mod workflow;
#[path = "supply_chain/yaml.rs"]
pub(crate) mod yaml;

const POLICY_PATH: &str = "tools/policies/supply_chain_policy.json";
const WORKFLOW_DIRECTORY: &str = ".github/workflows";

pub(crate) fn run(root: &Path, verify_cargo_graph: bool) -> Result<()> {
    run_at(
        root,
        &root.join(POLICY_PATH),
        &root.join(WORKFLOW_DIRECTORY),
        verify_cargo_graph,
    )
}

pub(crate) fn run_at(
    root: &Path,
    policy_path: &Path,
    workflow_directory: &Path,
    verify_cargo_graph: bool,
) -> Result<()> {
    let policy = policy::load_file(policy_path, chrono::Local::now().date_naive())
        .map_err(|error| PreservedFailure::new(format!("供应链策略无效：{error}"), Some(1)))?;
    let mut errors = local_patch::validate(root, &policy, None);
    errors.extend(workflow::validate(workflow_directory, &policy));
    if verify_cargo_graph {
        errors.extend(cargo_graph::verify(root, &policy));
    }
    if !errors.is_empty() {
        for error in &errors {
            eprintln!("供应链门禁失败：{error}");
        }
        return Err(
            PreservedFailure::new("供应链策略、固定版本和依赖图校验失败".into(), Some(1)).into(),
        );
    }
    println!("供应链策略、固定版本和依赖图校验通过");
    Ok(())
}

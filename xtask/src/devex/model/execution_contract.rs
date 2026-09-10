use super::{CacheState, DevexSuite};

pub(crate) const RESOURCE_GATE_TARGETED_ENV: &str = "RYFRAME_RESOURCE_GATE_TARGETED";
pub(crate) const RESOURCE_GATE_TARGETED_ACTIVATION: &str = "replay-verified-v1";
pub(crate) const RESOURCE_GATE_DECISION_ENV: &str = "RYFRAME_RESOURCE_GATE_DECISION_FILE";
pub(crate) const RESOURCE_GATE_DECISION_TEMPLATE: &str =
    "{target}/resource-gate-decision-{label}.json";

const STANDARD_COMPARISON_RUNS: usize = 6;
const CRITICAL_COMPARISON_RUNS: usize = 20;

impl DevexSuite {
    pub(crate) fn minimum_runs(self, variant: &str) -> usize {
        match self {
            Self::RustIncremental if variant == "application" => STANDARD_COMPARISON_RUNS,
            Self::RustColdBuild | Self::RustIncremental | Self::RustGate | Self::RustSccache => {
                CRITICAL_COMPARISON_RUNS
            }
            Self::CargoDevSave if !matches!(variant, "config-only" | "resource-manifest") => {
                CRITICAL_COMPARISON_RUNS
            }
            _ => STANDARD_COMPARISON_RUNS,
        }
    }

    pub(crate) fn validate_cache_state(self, observed: CacheState) -> Result<(), String> {
        let required = match self {
            Self::RustColdBuild => CacheState::Cold,
            Self::RustIncremental
            | Self::ResourceGate
            | Self::RustGate
            | Self::RustSccache
            | Self::RuntimeApi
            | Self::RuntimeJobs
            | Self::RuntimeTenants => CacheState::Warm,
            Self::CargoDevSave
            | Self::ResourceGenerator
            | Self::FrontendFast
            | Self::FrontendBuild
            | Self::RuntimeHomepage => return Ok(()),
        };
        (observed == required).then_some(()).ok_or_else(|| {
            format!(
                "suite `{}` 只允许 --cache {}，当前为 {}",
                self.as_str(),
                required.as_str(),
                observed.as_str()
            )
        })
    }
}

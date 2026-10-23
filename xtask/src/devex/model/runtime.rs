use super::{DevexSuite, StepDefinition, SuiteDefinition, SuiteRequirement, WorkingDirectory};

pub(super) fn definition(suite: DevexSuite, variant: &str) -> Result<SuiteDefinition, String> {
    let steps: &[StepDefinition] = match (suite, variant) {
        (DevexSuite::RuntimeHomepage, "default") => HOMEPAGE,
        (DevexSuite::RuntimeApi, "10" | "50" | "100") => API,
        (DevexSuite::RuntimeJobs, "10" | "50" | "100") => JOBS,
        (DevexSuite::RuntimeTenants, "10" | "50" | "100") => TENANTS,
        _ => return Err("运行时变体：homepage 使用 default，其余使用 10、50 或 100".into()),
    };
    Ok(SuiteDefinition {
        steps,
        features: &[],
        environment: &[],
        remove_environment: &[],
        requirement: SuiteRequirement::Frontend,
        requires_frontend: true,
    })
}

macro_rules! runtime_step {
    ($name:ident, $suite:literal) => {
        const $name: &[StepDefinition] = &[StepDefinition {
            working_directory: WorkingDirectory::RunnerFrontend,
            program: "__devex-runtime",
            args: &[
                "--suite",
                $suite,
                "--backend",
                "{backend}",
                "--frontend",
                "{frontend}",
                "--runner-frontend",
                "{runner-frontend}",
                "--output",
                "{target}/runtime-{label}.json",
            ],
        }];
    };
}

runtime_step!(HOMEPAGE, "homepage");
runtime_step!(API, "api");
runtime_step!(JOBS, "jobs");
runtime_step!(TENANTS, "tenants");

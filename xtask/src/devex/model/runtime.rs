use super::{DevexSuite, StepDefinition, SuiteDefinition, SuiteRequirement, WorkingDirectory};

pub(super) fn definition(suite: DevexSuite, variant: &str) -> Result<SuiteDefinition, String> {
    let (steps, environment): (&[StepDefinition], &[(&str, &str)]) = match (suite, variant) {
        (DevexSuite::RuntimeHomepage, "default") => (HOMEPAGE, &[]),
        (DevexSuite::RuntimeApi, "10" | "50" | "100") => (API, concurrency(variant)),
        (DevexSuite::RuntimeJobs, "10" | "50" | "100") => (JOBS, concurrency(variant)),
        (DevexSuite::RuntimeTenants, "10" | "50" | "100") => (TENANTS, concurrency(variant)),
        _ => return Err("运行时变体：homepage 使用 default，其余使用 10、50 或 100".into()),
    };
    Ok(SuiteDefinition {
        steps,
        features: &[],
        environment,
        remove_environment: &[],
        requirement: SuiteRequirement::Frontend,
        requires_frontend: true,
    })
}

fn concurrency(variant: &str) -> &'static [(&'static str, &'static str)] {
    match variant {
        "10" => &[("RYFRAME_DEVEX_CONCURRENCY", "10")],
        "50" => &[("RYFRAME_DEVEX_CONCURRENCY", "50")],
        "100" => &[("RYFRAME_DEVEX_CONCURRENCY", "100")],
        _ => unreachable!("已校验运行时并发"),
    }
}

macro_rules! runtime_step {
    ($name:ident, $suite:literal) => {
        const $name: &[StepDefinition] = &[StepDefinition {
            working_directory: WorkingDirectory::RunnerFrontend,
            program: "node",
            args: &[
                "{driver}/scripts/devex/runtime.mjs",
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

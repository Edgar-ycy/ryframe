use std::path::PathBuf;

#[path = "fixture_control/artifact.rs"]
mod artifact;
pub(crate) use artifact::*;
#[path = "fixture_control/dataset.rs"]
mod dataset;
pub(crate) use dataset::*;
#[path = "fixture_control/environment.rs"]
mod environment;
pub(crate) use environment::*;
#[path = "fixture_control/retention.rs"]
mod retention;
pub(crate) use retention::*;
#[path = "fixture_control/review.rs"]
mod review;
pub(crate) use review::*;
#[path = "fixture_control/request.rs"]
mod request;
pub(crate) use request::*;
#[path = "fixture_control/services.rs"]
mod services;
pub(crate) use services::*;
#[path = "fixture_control/source_pair.rs"]
mod source_pair;
pub(crate) use source_pair::*;
#[path = "fixture_control/successor.rs"]
mod successor;
pub(crate) use successor::*;

pub(crate) const FIXTURE_CONTROL_PROTOCOL_KIND: &str = "ryframe-reference-fixture-control";
pub(crate) const FIXTURE_CONTROL_USAGE: &str = concat!(
    "用法：cargo xtask check recovery fixture <子域> <操作与参数>\n",
    "  artifact snapshot --runtime-dir <目录> --job-id <正 i64> --receipt <新文件>\n",
    "  artifact verify-deleted --runtime-dir <目录> --job-id <正 i64> --receipt <文件>\n",
    "  dataset plan --environment <文件> --runtime <目录> --work-dir <新目录> --output <新文件> --side base|candidate --write\n",
    "  dataset prepare --environment <文件> --runtime <目录> --plan <文件> --side base|candidate --write\n",
    "  retention <inspect|plan-history|verify-cleaned> --runtime-dir <目录> --tenant tenant-xxxxxxxx --migration <正 i64>\n",
    "  retention historical-expired --runtime-dir <目录> --tenant tenant-xxxxxxxx --migration <正 i64> --plan-sha256 <SHA256> --write\n",
    "  retention export-backup --runtime-dir <目录> --tenant tenant-xxxxxxxx --migration <正 i64> --write\n",
    "  environment plan --review <文件> --fixture <文件> --maintenance-build <文件> [--side seed|base|candidate]\n",
    "  environment prepare --review <文件> --fixture <文件> --maintenance-build <文件> [--side seed|base|candidate] --output <新目录> [--secrets-dir <目录>] --write\n",
    "  environment review --review <文件> --output <新文件> --write\n",
    "  environment rotate-secrets --fixture <文件> --output <新目录> --write\n",
    "  environment bootstrap-secrets --source-fixture <文件> --source-secrets <目录> --fixture <文件> --write\n",
    "  review --template <文件> --fixture <文件> --future-root <新目录> --id <ID> --api-port <端口> --worker-port <端口> --frontend-port <端口> --rustfs-api-port <端口> --rustfs-console-port <端口> --redis-port <端口> --output <新文件> --write\n",
    "  request --environment <文件> --service-run <目录> --id <ID> --side seed|base|candidate --output <新文件> --write\n",
    "  source-pair --output <新文件> --write\n",
    "  successor relationship --source-result <文件> --predecessor-review <文件> --predecessor-request <文件> --successor-review <文件> --seed-request <文件> --base-request <文件> --candidate-request <文件> --id <ID> --output <新文件> --write\n",
    "  successor generation-request --successor <文件> --source-backend <目录> --expected-head <SHA> --backend-build <文件> --maintenance-build <文件> --source-environment <文件> --id <ID> [--adapter-contract <值>] [--product-backend <目录>] [--output <新文件> --write]\n",
    "  successor arm-request --successor <文件> --source-export-result <文件> --workspace <目录> --id <ID> --side base|candidate --copy-directory <新目录> [--output <新文件> --write]\n",
    "  services <rustfs|redis|buckets|close> --review <文件> --environment <文件> --write\n",
    "  services status --review <文件> --environment <文件>\n",
    "  services <reconcile|recover|restart> --review <文件> --environment <文件> --owner-binding <文件> --write\n",
    "路径必须是当前后端 .local-tests 内无链接的绝对路径；只读 plan/status 拒绝 --write，dataset plan、发布和生命周期操作要求 --write。",
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FixtureControlCommand {
    Artifact(FixtureArtifactCommand),
    Dataset(FixtureDatasetCommand),
    Environment(FixtureEnvironmentCommand),
    Retention(FixtureRetentionCommand),
    Review(FixtureReviewCommand),
    Request(FixtureRequestCommand),
    SourcePair(FixtureSourcePairCommand),
    Successor(FixtureSuccessorCommand),
    Services(FixtureServicesCommand),
}

impl FixtureControlCommand {
    pub(crate) const fn domain(&self) -> &'static str {
        match self {
            Self::Artifact(_) => "artifact",
            Self::Dataset(_) => "dataset",
            Self::Environment(_) => "environment",
            Self::Retention(_) => "retention",
            Self::Review(_) => "review",
            Self::Request(_) => "request",
            Self::SourcePair(_) => "source-pair",
            Self::Successor(_) => "successor",
            Self::Services(_) => "services",
        }
    }

    pub(crate) const fn operation(&self) -> &'static str {
        match self {
            Self::Artifact(command) => command.operation(),
            Self::Dataset(command) => command.operation(),
            Self::Environment(command) => command.operation(),
            Self::Retention(command) => command.operation(),
            Self::Review(command) => command.operation(),
            Self::Request(command) => command.operation(),
            Self::SourcePair(command) => command.operation(),
            Self::Successor(command) => command.operation(),
            Self::Services(command) => command.operation(),
        }
    }

    pub(crate) const fn writes(&self) -> bool {
        match self {
            Self::Artifact(command) => command.writes(),
            Self::Dataset(command) => command.writes(),
            Self::Environment(command) => command.writes(),
            Self::Retention(command) => command.writes(),
            Self::Review(command) => command.writes(),
            Self::Request(command) => command.writes(),
            Self::SourcePair(command) => command.writes(),
            Self::Successor(command) => command.writes(),
            Self::Services(command) => command.writes(),
        }
    }

    pub(crate) const fn is_help(&self) -> bool {
        matches!(
            self,
            Self::Artifact(FixtureArtifactCommand::Help)
                | Self::Dataset(FixtureDatasetCommand::Help)
                | Self::Environment(FixtureEnvironmentCommand::Help)
                | Self::Retention(FixtureRetentionCommand::Help)
                | Self::Review(FixtureReviewCommand::Help)
                | Self::Request(FixtureRequestCommand::Help)
                | Self::SourcePair(FixtureSourcePairCommand::Help)
                | Self::Successor(FixtureSuccessorCommand::Help)
                | Self::Services(FixtureServicesCommand::Help)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FixtureSide {
    Seed,
    Base,
    Candidate,
}

impl FixtureSide {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Seed => "seed",
            Self::Base => "base",
            Self::Candidate => "candidate",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FixtureFileOutput {
    pub(crate) output: PathBuf,
}

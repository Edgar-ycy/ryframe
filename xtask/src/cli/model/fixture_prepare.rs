use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FixturePrepareCommand {
    Help,
    Run(FixturePrepareOptions),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FixturePrepareOptions {
    pub(crate) output_dir: PathBuf,
    pub(crate) expected_sources: Option<FixtureExpectedSources>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FixtureExpectedSources {
    pub(crate) backend_sha: String,
    pub(crate) frontend_sha: String,
}

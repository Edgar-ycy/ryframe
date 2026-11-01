use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FullStackConfigOptions {
    pub template: PathBuf,
    pub runtime_dir: PathBuf,
    pub run_id: String,
    pub attempt: String,
    pub source_sha: String,
    pub source_backend: PathBuf,
    pub source_frontend: PathBuf,
    pub mysql_container: String,
    pub redis_container: String,
    pub s3_container: String,
    pub write: bool,
    pub plan: bool,
}

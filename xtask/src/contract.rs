//! OpenAPI 候选与正式契约同步入口。

#[path = "contract/candidate.rs"]
mod candidate;
#[path = "contract/formal.rs"]
mod formal;
#[path = "contract/model.rs"]
mod model;
#[path = "contract/transaction.rs"]
mod transaction;

#[allow(unused_imports)]
pub(crate) use candidate::generate_api;

#[allow(unused_imports)]
pub(crate) use candidate::{
    CANDIDATE_GENERATION_ARGS, apply_candidate, apply_candidate_with_staging_hook,
    generated_artifact_paths, validate_candidate_contract,
};
#[allow(unused_imports)]
pub(crate) use formal::{FORMAL_SYNC_ARGS, github_repository_identifier, validate_formal_sync};
#[allow(unused_imports)]
pub(crate) use model::{ContractFileOperations, Snapshot, sha256_hex};
#[allow(unused_imports)]
pub(crate) use transaction::{install_snapshots_with, write_atomically_with};

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
pub(crate) use candidate::{api_sync, run};

#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use candidate::{
    apply_candidate, apply_candidate_with_staging_hook, generated_artifact_paths,
    validate_candidate_contract,
};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use formal::{github_repository_identifier, validate_formal_sync};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use model::{ContractFileOperations, Snapshot, sha256_hex};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use transaction::{install_snapshots_with, write_atomically_with};

//! OpenAPI 候选与正式契约同步入口。

#[path = "contract/atomic.rs"]
mod atomic;
#[path = "contract/candidate.rs"]
mod candidate;
#[path = "contract/formal.rs"]
mod formal;
#[path = "contract/generator.rs"]
mod generator;
#[path = "contract/model.rs"]
pub(crate) mod model;
#[path = "contract/ownership.rs"]
mod ownership;
#[path = "contract/readonly.rs"]
mod readonly;
#[path = "contract/recovery.rs"]
mod recovery;
#[path = "contract/source.rs"]
mod source;
#[path = "contract/transaction.rs"]
mod transaction;

pub(crate) use candidate::canonical_contract;

#[allow(unused_imports)]
pub(crate) use candidate::generate_api;

#[allow(unused_imports)]
pub(crate) use atomic::write_atomically_with;
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use candidate::{apply_candidate, apply_candidate_with_staging_hook};
#[allow(unused_imports)]
pub(crate) use candidate::{generated_artifact_paths, validate_candidate_contract};
#[allow(unused_imports)]
pub(crate) use formal::{
    formal_sync_arguments, github_repository_identifier, validate_formal_sync,
};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use generator::GeneratorEntry;
#[allow(unused_imports)]
pub(crate) use model::{ContractFileOperations, Snapshot, sha256_hex};
#[allow(unused_imports)]
pub(crate) use readonly::check_current_with;
#[allow(unused_imports)]
pub(crate) use source::verify_contract_source;
#[allow(unused_imports)]
pub(crate) use transaction::install_snapshots_with;

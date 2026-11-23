use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::{
    Result,
    cli::FixtureSuccessorRelationship,
    recovery::clone::native::{
        cache::binding::source::relationship, json as document, resources::common,
    },
};

use super::output_value;

#[path = "successor/arm.rs"]
pub(super) mod arm;
#[path = "successor/arm_input.rs"]
pub(super) mod arm_input;
#[path = "successor/arm_target.rs"]
pub(crate) mod arm_target;

pub(super) fn publish_relationship(
    root: &Path,
    options: &FixtureSuccessorRelationship,
) -> Result<Value> {
    output_value(&options.output, root, "successor relationship 输出")?;
    let output = options.output.components().collect::<PathBuf>();
    let source = common::common_file(root, &options.source_result, "successor 来源结果")?
        .components()
        .collect::<PathBuf>();
    let bind = |path: &Path| relationship::canonical_binding(root, path);
    let inputs = json!({
        "id": options.id, "source_result": common::binding(&source)?,
        "predecessor_review": bind(&options.predecessor_review)?,
        "predecessor_request": bind(&options.predecessor_request)?,
        "successor_review": bind(&options.successor_review)?,
        "requests": {
            "seed": bind(&options.seed_request)?,
            "base": bind(&options.base_request)?,
            "candidate": bind(&options.candidate_request)?,
        },
    });
    let value = relationship::build(root, &inputs)?;
    document::write_new(&output, &value)?;
    let published = common::binding(&output)?;
    if relationship::load(root, &published)? != value {
        return Err("successor 证据发布后无法重算；保留原输出，禁止重放".into());
    }
    Ok(json!({
        "status":"seed_review_successor_published",
        "relationship_sha256":value["relationship_sha256"],
        "remote_writes":0,
    }))
}

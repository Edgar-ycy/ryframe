use std::collections::BTreeSet;

use ryframe_api::generated::crud_resources_extension;

#[path = "api_contracts/snapshot_export.rs"]
mod snapshot_export;

#[test]
fn generated_resource_metadata_is_sorted_and_unique() {
    let document = crud_resources_extension();
    let resources = document["resources"]
        .as_array()
        .expect("生成资源扩展必须包含资源数组");
    let names = resources
        .iter()
        .map(|resource| resource["name"].as_str().expect("生成资源名称必须是字符串"))
        .collect::<Vec<_>>();
    assert!(names.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(
        names.len(),
        names.iter().copied().collect::<BTreeSet<_>>().len()
    );
}

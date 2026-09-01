use std::{collections::BTreeMap, path::PathBuf};

use ryframe_generator::{load_resource, render_resources};

fn device() -> ryframe_generator::ResourceIr {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/device.toml");
    load_resource(path).expect("Device 资源清单应有效")
}

#[test]
fn generated_outputs_ignore_resource_source_hashes() {
    let original = device();
    let mut changed = original.clone();
    changed.source_hash = "source-hash-for-a-non-structural-change".to_owned();
    let before = render_resources(&[original]).expect("原始资源应能生成");
    let after = render_resources(&[changed]).expect("变更资源应能生成");
    let contents = |assets: Vec<ryframe_generator::GeneratedAsset>| {
        assets
            .into_iter()
            .map(|asset| ((asset.root, asset.path), asset.content))
            .collect::<BTreeMap<_, _>>()
    };
    assert_eq!(
        contents(before.assets),
        contents(after.assets),
        "生成源码不得仅因资源来源哈希变化"
    );
}

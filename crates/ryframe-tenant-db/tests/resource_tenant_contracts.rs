use std::collections::BTreeSet;

const GENERATED_REGISTRY: &str = include_str!("../src/generated/mod.rs");

#[test]
fn generated_migration_registry_is_complete_sorted_and_unique() {
    let names = migration_names(GENERATED_REGISTRY);
    assert!(names.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(
        names.len(),
        names.iter().copied().collect::<BTreeSet<_>>().len()
    );
    assert_eq!(
        names.len(),
        GENERATED_REGISTRY.matches("Box::new(").count(),
        "迁移名称与构造器注册必须一一对应"
    );
}

fn migration_names(source: &str) -> Vec<&str> {
    let body = source
        .split_once("pub const MIGRATION_NAMES: &[&str] = &[")
        .and_then(|(_, suffix)| suffix.split_once("];"))
        .map(|(body, _)| body)
        .expect("生成迁移注册表必须声明 MIGRATION_NAMES");
    body.split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .expect("生成迁移名称必须使用字符串字面量")
        })
        .collect()
}

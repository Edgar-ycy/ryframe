use std::{fs, path::Path};

use ryframe_generator::{
    IdGeneration, IndexSpec, ResourceSpec, ValueType, normalize_resource, render_resources,
};

pub(super) fn prepare(backend: &Path) {
    let tests = backend.join("crates/ryframe-application/tests");
    for (module, auto) in [("unsigned_auto", true), ("unsigned_snowflake", false)] {
        let mut spec =
            ResourceSpec::parse(include_str!("../fixtures/order.toml"), "order.toml").unwrap();
        spec.fields
            .iter_mut()
            .find(|field| field.name == "id")
            .unwrap()
            .value_type = ValueType::U64;
        if auto {
            spec.database.id_generation = IdGeneration::AutoIncrement;
            spec.database.indexes.push(IndexSpec {
                name: "idx_order_auto_id".into(),
                fields: vec!["id".into()],
                unique: false,
            });
        }
        if !auto {
            for field in spec
                .fields
                .iter_mut()
                .filter(|field| matches!(field.name.as_str(), "created_at" | "updated_at"))
            {
                field.sql_type = Some("DATETIME(3)".into());
                field.sql_default = Some("CURRENT_TIMESTAMP(3)".into());
                if field.name == "updated_at" {
                    field.sql_on_update = Some("CURRENT_TIMESTAMP(3)".into());
                }
            }
        }
        let resource = normalize_resource(spec, "order.toml", "unsigned-fake").unwrap();
        let generated = render_resources(&[resource]).unwrap();
        let root = tests.join(module);
        fs::create_dir_all(&root).unwrap();
        for name in ["model", "port", "fake"] {
            let source = &generated
                .assets
                .iter()
                .find(|asset| {
                    asset.path
                        == format!("crates/ryframe-application/src/generated/order/{name}.rs")
                })
                .unwrap()
                .content;
            fs::write(root.join(format!("{name}.rs")), source).unwrap();
        }
        fs::write(
            root.join("mod.rs"),
            "pub mod fake;\npub mod model;\npub mod port;\n",
        )
        .unwrap();
    }
    fs::write(
        tests.join("generated_unsigned_fake.rs"),
        include_str!("../fixtures/unsigned_fake_contract.rs"),
    )
    .unwrap();
}

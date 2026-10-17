use std::{collections::BTreeSet, fs, path::Path, process::Command};

use super::*;
use crate::{ResourceSpec, normalize_resource};

fn order() -> ResourceIr {
    let spec = ResourceSpec::parse(include_str!("../fixtures/order.toml"), "order.toml").unwrap();
    normalize_resource(spec, "order.toml", "test").unwrap()
}

fn wide(count: usize) -> ResourceIr {
    let mut resource = order();
    resource.module = "business".into();
    let name = resource
        .fields
        .iter()
        .find(|field| field.name == "name")
        .unwrap()
        .clone();
    for index in 0..count {
        let mut field = name.clone();
        field.name = format!("value_{index}");
        field.column = field.name.clone();
        field.labels.zh_cn = format!("字段{index}");
        field.labels.en = format!("Field {index}");
        field.usage.create = index % 7 != 1;
        field.usage.update = index % 7 != 2;
        field.usage.create_optional = index % 7 == 3;
        field.usage.update_optional = index % 7 == 3;
        field.nullable = index % 7 == 4;
        field.value_type = match index % 5 {
            0 => ValueType::Decimal,
            1 => ValueType::I64,
            2 => ValueType::U64,
            3 => ValueType::I32,
            _ => ValueType::String,
        };
        field.wire_type = if field.value_type == ValueType::I32 {
            ValueType::I32
        } else {
            ValueType::String
        };
        field.typescript_type = field.wire_type.typescript_type(field.nullable);
        field.widget = if field.value_type == ValueType::I32 {
            WidgetIr::Number
        } else {
            WidgetIr::Text
        };
        if index % 11 == 0 {
            let status = resource
                .fields
                .iter()
                .find(|field| field.name == "status")
                .unwrap();
            field.enum_values = status.enum_values.clone();
            field.widget = WidgetIr::Select;
        }
        resource.fields.push(field);
    }
    resource
}

fn assets(resource: &ResourceIr) -> Vec<crate::GeneratedAsset> {
    crate::resource::business_frontend_assets(std::slice::from_ref(resource)).unwrap()
}

#[test]
fn create_only_fields_disable_editing_for_every_form_widget() {
    let mut resource = wide(24);
    resource
        .fields
        .iter_mut()
        .find(|field| field.name == "value_0")
        .unwrap()
        .usage
        .update = false;
    let text = resource
        .fields
        .iter()
        .find(|field| field.name == "value_2")
        .unwrap();
    let number = resource
        .fields
        .iter()
        .find(|field| field.name == "value_23")
        .unwrap();
    let radio = resource
        .fields
        .iter()
        .find(|field| field.name == "value_0")
        .unwrap();
    for (field, kind) in [(text, "text"), (number, "number"), (radio, "radio")] {
        assert!(field.usage.create && !field.usage.update);
        assert!(presentation::form_properties(field).contains(&format!("kind: {kind:?}")));
        assert!(presentation::form_properties(field).contains(&"disabledOnEdit: true".into()));
        let mut editable = field.clone();
        editable.usage.update = true;
        assert!(!presentation::form_properties(&editable).contains(&"disabledOnEdit: true".into()));
    }
    let count = assets(&resource)
        .iter()
        .filter(|asset| asset.path.contains("/form-fields"))
        .map(|asset| asset.content.matches("disabledOnEdit: true").count())
        .sum::<usize>();
    assert_eq!(
        count,
        resource
            .fields
            .iter()
            .filter(|field| field.usage.create && !field.usage.update)
            .count()
    );
}

fn import_like_labels() -> ResourceIr {
    let mut resource = wide(8);
    let field = resource
        .fields
        .iter_mut()
        .find(|field| field.name == "name")
        .unwrap();
    field.labels.zh_cn = "hello from '../path".into();
    field.labels.en = "import { sample } from '../path".into();
    let labels = field.labels.clone();
    let status = resource
        .fields
        .iter_mut()
        .find(|field| field.name == "status")
        .unwrap();
    *status.enum_values.values_mut().next().unwrap() = labels;
    resource
}

#[test]
fn single_leaf_modules_preserve_import_like_labels_and_placeholders() {
    let resource = import_like_labels();
    assert!(presentation::fields(&resource).lines().count() > 300);
    let generated = assets(&resource);
    assert_local_imports_exist(&generated);
    for suffix in [
        "columns.ts",
        "query.ts",
        "form-fields.ts",
        "options/status.ts",
    ] {
        let asset = generated
            .iter()
            .find(|asset| asset.path.ends_with(&format!("/{suffix}")))
            .unwrap();
        assert!(asset.content.contains(
            "label: translate(\"hello from '../path\", \"import { sample } from '../path\")"
        ));
        assert!(!asset.content.contains("hello from './path"));
    }
    for (suffix, placeholder) in [
        ("query.ts", "请输入或选择hello from '../path"),
        ("form-fields.ts", "请输入hello from '../path"),
    ] {
        let asset = generated
            .iter()
            .find(|asset| asset.path.ends_with(&format!("/{suffix}")))
            .unwrap();
        assert!(
            asset
                .content
                .contains(&format!("placeholder: translate({placeholder:?},"))
        );
    }
    assert!(
        !generated
            .iter()
            .any(|asset| asset.path.contains("/columns/"))
    );
}

#[test]
fn wide_models_preserve_every_field_and_repeat_without_differences() {
    for count in [8, 24, 25, 64, 257, 1000] {
        let resource = wide(count);
        let first = assets(&resource);
        assert_eq!(first, assets(&resource));
        assert_local_imports_exist(&first);
        let names = first
            .iter()
            .map(|asset| &asset.path)
            .collect::<BTreeSet<_>>();
        assert_eq!(names.len(), first.len());
        assert!(first.iter().all(|asset| asset.content.lines().count()
            <= if asset.path.ends_with(".vue") {
                600
            } else {
                300
            }));
        let joined = first
            .iter()
            .map(|asset| asset.content.as_str())
            .collect::<String>();
        for field in &resource.fields {
            if field.usage.list
                || (field.usage.filter && !matches!(field.widget, WidgetIr::Hidden))
                || field.usage.create
                || field.usage.update
            {
                assert!(
                    joined.contains(&format!("key: {:?}", field.name)),
                    "缺少字段 {}",
                    field.name
                );
            }
            if field.usage.create || field.usage.update {
                assert!(joined.contains(&format!("  {}: {}", field.name, field.typescript_type)));
                assert!(joined.contains(&format!("record.{}", field.name)));
            }
        }
        assert_eq!(
            transport::page(&resource),
            first
                .iter()
                .find(|asset| asset.path.ends_with("/page.vue"))
                .unwrap()
                .content
        );
        assert_eq!(
            transport::registration(&resource),
            first
                .iter()
                .find(|asset| asset.path.ends_with("/registration.ts"))
                .unwrap()
                .content
        );
        assert!(joined.contains("disabledOnEdit: true"));
        assert!(joined.contains("editOnly: true"));
        assert!(joined.contains("form.value_11 === '' ? null : form.value_11") || count < 12);
        assert!(joined.contains("form.value_10 === '' ? undefined : form.value_10") || count < 11);
    }
}

#[test]
fn modules_are_emitted_only_after_the_single_file_exceeds_300_lines() {
    let mut compact = false;
    let mut split = false;
    for count in 0..20 {
        let resource = wide(count);
        let original = presentation::fields(&resource);
        let generated = fields(&resource);
        if original.lines().count() <= 300 {
            compact = true;
            assert_eq!(generated, vec![("fields.ts".into(), original)]);
        } else {
            split = true;
            assert!(generated.len() > 1);
            assert!(
                generated
                    .iter()
                    .all(|(_, content)| content.lines().count() <= 300)
            );
        }
    }
    assert!(compact && split, "必须覆盖行数阈值的两侧");
}

#[test]
fn huge_enums_and_empty_forms_use_bounded_modules() {
    let mut resource = wide(0);
    let status = resource
        .fields
        .iter_mut()
        .find(|field| field.name == "status")
        .unwrap();
    let labels = status.labels.clone();
    status.enum_values = (0..1641)
        .map(|index| (index.to_string(), labels.clone()))
        .collect();
    for field in &mut resource.fields {
        field.usage.create = false;
        field.usage.update = false;
    }
    let generated = assets(&resource);
    assert!(
        generated
            .iter()
            .any(|asset| asset.path.contains("options/status/group-0"))
    );
    assert!(
        generated
            .iter()
            .any(|asset| asset.content.contains("value: 1640"))
    );
    assert_eq!(generated, assets(&resource));
    assert_local_imports_exist(&generated);
}

fn assert_local_imports_exist(assets: &[crate::GeneratedAsset]) {
    let paths = assets
        .iter()
        .map(|asset| Path::new(&asset.path))
        .collect::<BTreeSet<_>>();
    for asset in assets {
        for line in asset.content.lines() {
            let Some((_, path)) = line.split_once(" from '") else {
                continue;
            };
            let Some(path) = path.strip_suffix('\'') else {
                continue;
            };
            if !path.starts_with('.') {
                continue;
            }
            let mut target = asset
                .path
                .rsplit_once('/')
                .unwrap()
                .0
                .split('/')
                .collect::<Vec<_>>();
            for segment in path.split('/') {
                match segment {
                    "." => {}
                    ".." => {
                        target.pop();
                    }
                    value => target.push(value),
                }
            }
            let target = target.join("/");
            assert!(
                paths.contains(Path::new(&target))
                    || paths.contains(Path::new(&format!("{target}.ts"))),
                "{} 引用缺失 {target}",
                asset.path
            );
        }
    }
}

#[test]
fn date_formatter_is_emitted_only_for_visible_datetime_columns() {
    let mut resource = order();
    assert!(presentation::fields(&resource).contains("formatDate: (value: string) => string"));
    assert!(transport::page(&resource).contains("Presentation(translate, formatLocalizedDate)"));
    for field in &mut resource.fields {
        if field.value_type == ValueType::DateTime {
            field.usage.list = false;
        }
    }
    assert!(!presentation::fields(&resource).contains("formatDate"));
    assert!(!transport::page(&resource).contains("formatLocalizedDate"));
    assert!(transport::page(&resource).contains("Presentation(translate)"));
}

#[test]
fn small_managed_resources_keep_the_single_file_layout() {
    for name in ["post", "notice"] {
        let source = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("../../catalog/resources/{name}.toml")),
        )
        .unwrap();
        let spec = ResourceSpec::parse(&source, "fixture").unwrap();
        let resource =
            normalize_resource(spec, format!("catalog/resources/{name}.toml"), "test").unwrap();
        let expected = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("tests/fixtures/{name}_fields.ts.part")),
        )
        .unwrap()
        .replace("\r\n", "\n");
        let expected = format!(
            "// @generated by RyFrame {}\n{}",
            crate::GENERATOR_VERSION,
            expected.split_once('\n').unwrap().1
        );
        assert_eq!(fields(&resource), vec![("fields.ts".into(), expected)]);
    }
    let mut device = order();
    device.name = "device".into();
    device.pascal_name = "Device".into();
    assert_eq!(
        fields(&device),
        vec![("fields.ts".into(), presentation::fields(&device))]
    );
}

#[test]
#[ignore = "需要通过 RYFRAME_RESOURCE_WORKSPACE_FRONTEND_DIR 指定已安装依赖的前端，只读取类型及 TypeScript 编译器"]
fn generated_modules_typecheck_and_execute_with_real_frontend_types() {
    let frontend =
        std::env::var_os("RYFRAME_RESOURCE_WORKSPACE_FRONTEND_DIR").expect("应显式指定前端来源");
    let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.local-tests/frontend-render");
    fs::create_dir_all(&parent).unwrap();
    let temp = tempfile::tempdir_in(parent).unwrap();
    let mut empty = order();
    empty.module = "business".into();
    let status = empty
        .fields
        .iter_mut()
        .find(|field| field.name == "status")
        .unwrap();
    let labels = status.labels.clone();
    status.enum_values = (0..1641)
        .map(|index| (index.to_string(), labels.clone()))
        .collect();
    for field in &mut empty.fields {
        field.usage.create = false;
        field.usage.update = false;
        field.usage.filter = false;
        if field.value_type == ValueType::DateTime {
            field.usage.list = false;
        }
    }
    for (name, resource) in [
        ("wide", wide(1000)),
        ("empty", empty),
        ("labels", import_like_labels()),
    ] {
        let root = temp.path().join(name);
        for asset in assets(&resource) {
            let path = root.join(asset.path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, asset.content).unwrap();
        }
        fs::write(
            root.join("resource.json"),
            serde_json::to_vec(&resource).unwrap(),
        )
        .unwrap();
        let output = Command::new("node")
            .arg(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/frontend_typecheck.mjs"),
            )
            .arg(&frontend)
            .arg(&root)
            .output()
            .unwrap();
        if !output.status.success() {
            let evidence = temp.keep();
            panic!(
                "类型验证失败，现场保留于 {}：\n{}\n{}",
                evidence.display(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        println!("{}", String::from_utf8_lossy(&output.stdout).trim());
    }
}

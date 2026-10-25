use std::{fs, path::Path};

use super::snapshot::SourceSnapshot;

pub(super) fn verify(
    backend: &Path,
    frontend: Option<&Path>,
    business: &str,
    cases: &[(&str, &str)],
    mut execute: impl FnMut(bool) -> Result<(), String>,
) {
    let path = backend.join("crates/ryframe/src/business.rs");
    let original = fs::read_to_string(&path).unwrap();
    for &(case, expected) in cases {
        let source = registration(&original, business, case);
        fs::write(&path, source).unwrap();
        let before = SourceSnapshot::capture(backend).unwrap();
        let before_front = frontend.map(|root| SourceSnapshot::capture(root).unwrap());
        for sync in [false, true] {
            if sync && frontend.is_none() {
                continue;
            }
            let error = execute(sync).expect_err("跨模块冲突必须在正式安装前拒绝");
            assert!(error.contains(expected), "{case}, sync={sync}: {error}");
            assert!(error.contains("本批未安装"), "{error}");
            before.verify().unwrap();
            if let Some(before) = &before_front {
                before.verify().unwrap();
            }
        }
    }
    fs::write(path, original).unwrap();
}

fn registration(original: &str, business: &str, case: &str) -> String {
    let resource = if case == "resource" {
        "fixture_first"
    } else {
        "fixture_second"
    };
    let table = if case == "table" {
        "fixture_first"
    } else {
        "fixture_second"
    };
    let route = if case == "route" {
        "/fixture/first"
    } else {
        "/fixture/second"
    };
    let module = if case == "permission" {
        "fixture_first"
    } else {
        "fixture-second"
    };
    let shared = case == "permission";
    let entries = format!(
        "vec![fixture_conflict_module(\"fixture-first\", \"fixture_first\", \"fixture_first\", \"/fixture/first\", {shared}), fixture_conflict_module({module:?}, {resource:?}, {table:?}, {route:?}, {shared}),"
    );
    assert!(original.contains("vec!["), "夹具应保留显式业务注册表");
    format!(
        "{}\n{}",
        original.replacen("vec![", &entries, 1),
        MODULE.replace("BUSINESS_CRATE", business)
    )
}

// 使用正式组合根可登记的手写模块，复用实际模型描述符和 SDK 校验，不改受管生成文件。
const MODULE: &str = r#"
fn fixture_conflict_module(
    module: &'static str, name: &'static str, table: &'static str,
    route: &'static str, shared: bool,
) -> ryframe_sdk::RyFrameBusinessModule {
    let original = BUSINESS_CRATE::module();
    let mut descriptor = original.resources()[0];
    descriptor.name = name;
    descriptor.table = table;
    descriptor.route = Some(route);
    let tables = Box::leak(vec![ryframe_sdk::BusinessControlTableDescriptor {
        table, schema_canonical: original.control_tables()[0].schema_canonical,
    }].into_boxed_slice());
    let namespace = module.replace('_', "-");
    let resource = name.replace('_', "-");
    let extra = if shared { "\n[resources.extension_permissions]\nuse_shared='fixture-first:shared:use'\n" } else { "" };
    let catalog = format!("version=1\n[[resources]]\nname={name:?}\nmodule='business'\nsystem_tenant_only=true\n[resources.labels]\nzh_cn='夹具'\nen='Fixture'\n[resources.menu]\nkey='{namespace}.{resource}'\nparent='system'\norder=10\n[resources.menu.labels]\nzh_cn='夹具'\nen='Fixture'\n[resources.route]\nkey='{namespace}.{resource}'\npath={route:?}\n[resources.permissions]\ncreate='{namespace}:{resource}:add'\nread='{namespace}:{resource}:list'\nlist='{namespace}:{resource}:list'\nupdate='{namespace}:{resource}:edit'\ndelete='{namespace}:{resource}:remove'\n{extra}");
    ryframe_sdk::BusinessModuleBuilder::new(module)
        .resources([descriptor]).control_tables(tables)
        .access_catalog(Box::leak(catalog.into_boxed_str())).build()
}
"#;

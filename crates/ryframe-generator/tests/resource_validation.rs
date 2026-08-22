use std::{fs, path::PathBuf};

use ryframe_generator::{ResourceSpec, normalize_resource, render_resources};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(name)
}

fn device_source() -> String {
    fs::read_to_string(fixture("tests/fixtures/device.toml")).expect("应读取 Device fixture")
}

fn post_source() -> String {
    fs::read_to_string(fixture("../../catalog/resources/post.toml")).expect("应读取 Post 清单")
}

fn normalize(source: &str, resource: &str) -> Result<ryframe_generator::ResourceIr, String> {
    let path = format!("catalog/resources/{resource}.toml");
    let spec = ResourceSpec::parse(source, &path).map_err(|error| error.to_string())?;
    normalize_resource(spec, path, "test-source-hash").map_err(|error| error.to_string())
}

fn changed(source: &str, from: &str, to: &str) -> String {
    assert!(source.contains(from), "测试替换源不存在：{from}");
    source.replacen(from, to, 1)
}

#[test]
fn editable_widgets_and_patterns_fail_before_rendering() {
    let source = device_source();
    let unsupported = changed(
        &source,
        "name = \"name\"\nvalue_type = \"string\"\norder = 30\nwidget = \"text\"",
        "name = \"name\"\nvalue_type = \"string\"\norder = 30\nwidget = \"textarea\"",
    );
    let error = normalize(&unsupported, "device").expect_err("可编辑 textarea 必须被拒绝");
    assert!(error.contains("字段 name"));
    assert!(error.contains("frontend extension"));

    let pattern = changed(
        &source,
        "min_length = 1\nmax_length = 100",
        "min_length = 1\nmax_length = 100\npattern = \"^[a-z]+$\"",
    );
    let error = normalize(&pattern, "device").expect_err("未生成的 pattern 必须被拒绝");
    assert!(error.contains("validation.pattern"));
    assert!(error.contains("强类型 DTO"));
}

#[test]
fn defaults_soft_delete_and_enum_keys_are_strictly_typed() {
    let device = device_source();
    let wrong_default = changed(&device, "default = 1", "default = \"1\"");
    let error = normalize(&wrong_default, "device").expect_err("i32 默认值不得使用字符串");
    assert!(error.contains("字段 status"));
    assert!(error.contains("value_type=I32"));

    let wrong_soft_delete = changed(&device, "active = 0", "active = \"0\"");
    let error = normalize(&wrong_soft_delete, "device").expect_err("软删值必须同字段类型");
    assert!(error.contains("字段 del_flag"));
    assert!(error.contains("soft_delete.active"));

    let wrong_enum_key = changed(
        &device,
        "[fields.enum_values.\"0\"]",
        "[fields.enum_values.\"zero\"]",
    );
    let error = normalize(&wrong_enum_key, "device").expect_err("数字枚举键不得退化成字符串");
    assert!(error.contains("字段 status"));
    assert!(error.contains("枚举键 `zero`"));

    let post = post_source();
    let outside_enum = changed(&post, "default = \"1\"", "default = \"9\"");
    let error = normalize(&outside_enum, "post").expect_err("默认值必须属于有限枚举");
    assert!(error.contains("字段 status"));
    assert!(error.contains("不在 enum_values"));

    let decimal = changed(
        &device,
        "name = \"name\"\nvalue_type = \"string\"",
        "name = \"name\"\nvalue_type = \"decimal\"",
    );
    let error = normalize(&decimal, "device").expect_err("decimal 必须在 normalize 阶段失败");
    assert!(error.contains("字段 name"));
    assert!(error.contains("decimal"));
}

#[test]
fn v1_structural_assumptions_are_manifest_errors_not_renderer_panics() {
    let source = device_source();
    let cases = [
        (
            changed(&source, "module = \"system\"", "module = \"inventory\""),
            "仅支持 system 模块",
        ),
        (
            changed(
                &source,
                "path = \"/api/v1/system/devices\"",
                "path = \"/api/v1/devices\"",
            ),
            "/api/v1/system/<resources>",
        ),
        (
            changed(
                &source,
                "path = \"/api/v1/system/devices\"",
                "path = \"/api/v1/system/devices/{id}\"",
            ),
            "无占位符",
        ),
        (
            changed(
                &source,
                "primary_key = [\"tenant_id\", \"id\"]",
                "primary_key = [\"id\", \"tenant_id\"]",
            ),
            "租户字段开头",
        ),
        (
            changed(
                &source,
                "name = \"id\"\nvalue_type = \"i64\"\nwire_type = \"string\"",
                "name = \"id\"\nvalue_type = \"i32\"",
            ),
            "id 必须是非空 i64",
        ),
        (
            changed(
                &source,
                "fields = [\"tenant_id\", \"name\"]",
                "fields = [\"name\"]",
            ),
            "未包含租户字段",
        ),
    ];
    for (invalid, expected) in cases {
        let error = normalize(&invalid, "device").expect_err("结构前提必须在 IR 拒绝");
        assert!(error.contains(expected), "错误未包含 {expected}: {error}");
        assert!(error.contains("catalog/resources/device.toml"));
    }
}

#[test]
fn service_managed_and_form_view_contracts_are_explicit() {
    let device = device_source();
    let editable_tenant = changed(
        &device,
        "name = \"tenant_id\"\nvalue_type = \"string\"\norder = 10\nwidget = \"hidden\"\n\n[fields.usage]\nread = true\nfilter = true",
        "name = \"tenant_id\"\nvalue_type = \"string\"\norder = 10\nwidget = \"text\"\n\n[fields.usage]\ncreate = true\nread = true\nfilter = true",
    );
    let error = normalize(&editable_tenant, "device").expect_err("tenant_id 不得来自表单");
    assert!(error.contains("服务管理字段"));
    assert!(error.contains("字段 tenant_id"));

    let wrong_audit_type = changed(
        &device,
        "name = \"created_at\"\nvalue_type = \"date_time\"",
        "name = \"created_at\"\nvalue_type = \"string\"",
    );
    let error = normalize(&wrong_audit_type, "device").expect_err("审计时间类型必须固定");
    assert!(error.contains("字段 created_at"));
    assert!(error.contains("date_time"));

    let created_by = changed(
        &device,
        "created_at = \"created_at\"\nupdated_at = \"updated_at\"",
        "created_at = \"created_at\"\ncreated_by = \"id\"\nupdated_at = \"updated_at\"",
    );
    let error = normalize(&created_by, "device").expect_err("未实现操作者字段不得静默通过");
    assert!(error.contains("created_by/updated_by"));

    let post = post_source();
    let no_default = changed(
        &post,
        "default = \"1\"\nwidget = \"select\"",
        "widget = \"select\"",
    );
    let error = normalize(&no_default, "post").expect_err("非空 update-only 字段需要默认值");
    assert!(error.contains("字段 status"));
    assert!(error.contains("稳定默认值"));

    let no_view = changed(
        &post,
        "create = true\nread = true\nlist = true\nfilter = true",
        "create = true\nfilter = true",
    );
    let error = normalize(&no_view, "post").expect_err("表单字段必须存在于强类型视图");
    assert!(error.contains("字段 code"));
    assert!(error.contains("read/list"));
}

#[test]
fn generated_dto_validates_nonempty_strings_and_finite_enums() {
    let source = changed(
        &post_source(),
        "min_length = 1\nmax_length = 64",
        "max_length = 64",
    );
    let post = normalize(&source, "post").expect("缺省 min_length 的 required String 仍应可生成");
    let generated = render_resources(&[post]).expect("Post 应生成");
    let dto = generated
        .assets
        .iter()
        .find(|asset| asset.path.ends_with("post/dto.rs"))
        .expect("应生成 Post DTO")
        .content
        .as_str();
    assert!(dto.contains("length(min = 1, max = 64)"));
    assert!(dto.contains("fn validate_status_enum(value: &str)"));
    assert!(dto.contains("custom(function = \"validate_status_enum\")"));
    assert!(dto.contains("invalid_status"));

    let fields = generated
        .assets
        .iter()
        .find(|asset| asset.path.ends_with("post/fields.ts"))
        .expect("应生成 Post 表单字段")
        .content
        .as_str();
    assert!(fields.contains("key: \"status\",\n      kind: \"radio\""));
}

#[test]
fn mutable_control_unique_fields_exclude_the_current_record_on_update() {
    let source = changed(
        &post_source(),
        "create = true\nread = true\nlist = true\nfilter = true",
        "create = true\nupdate = true\nread = true\nlist = true\nfilter = true",
    );
    let post = normalize(&source, "post").expect("可变唯一字段清单应有效");
    let generated = render_resources(&[post]).expect("应生成可变唯一字段切片");
    let content = |suffix: &str| {
        generated
            .assets
            .iter()
            .find(|asset| asset.path.ends_with(suffix))
            .unwrap_or_else(|| panic!("缺少生成资产 {suffix}"))
            .content
            .as_str()
    };
    assert!(
        content("post/service.rs")
            .contains("find_by_code_for_update(tenant_id, &record.code, Some(id))")
    );
    assert!(content("post/port.rs").contains("exclude_id: Option<i64>"));
    assert!(
        content("post/repository.rs")
            .contains("select = select.filter(entity::Column::Id.ne(exclude_id))")
    );
}

#[test]
fn access_catalog_contains_lossless_menu_labels_and_named_extension_permissions() {
    let post = normalize(&post_source(), "post").expect("Post 清单应有效");
    assert_eq!(
        post.extension_permissions.get("export").map(String::as_str),
        Some("system:post:export")
    );
    let generated = render_resources(&[post]).expect("Post 应生成");
    let access = generated
        .assets
        .iter()
        .find(|asset| asset.path == "catalog/access.generated.toml")
        .expect("应生成权限目录")
        .content
        .as_str();
    let value = toml::from_str::<toml::Value>(access).expect("权限目录应是有效 TOML");
    let post = &value["resources"][0];
    assert_eq!(value["version"].as_integer(), Some(1));
    assert_eq!(post["labels"]["zh_cn"].as_str(), Some("岗位"));
    assert_eq!(post["menu"]["parent"].as_str(), Some("system"));
    assert_eq!(post["menu"]["order"].as_integer(), Some(8));
    assert_eq!(
        post["extension_permissions"]["export"].as_str(),
        Some("system:post:export")
    );

    let invalid = changed(
        &post_source(),
        "export_permission = \"system:post:export\"",
        "export_permission = 7",
    );
    let error = normalize(&invalid, "post").expect_err("扩展权限必须严格为权限字符串");
    assert!(error.contains("export_permission"));
    assert!(error.contains("必须是字符串"));

    let device = normalize(&device_source(), "device").expect("Device 清单应有效");
    let generated = render_resources(&[device]).expect("Device 应生成");
    let openapi = generated
        .assets
        .iter()
        .find(|asset| asset.path.ends_with("device/openapi.rs"))
        .expect("应生成 Device OpenAPI")
        .content
        .as_str();
    assert!(openapi.contains("\"extension_permissions\": {}"));
}

#[test]
fn permissions_use_three_strict_kebab_case_segments() {
    let source = device_source()
        .replacen("name = \"device\"", "name = \"work_order\"", 1)
        .replace("system:device:", "system:work-order:");
    normalize(&source, "work_order").expect("snake_case 资源可显式使用 kebab-case 权限段");

    let invalid = source.replace("system:work-order:list", "system:work_order:list");
    let error = normalize(&invalid, "work_order").expect_err("权限值不得包含下划线");
    assert!(error.contains("权限标识格式无效"));
    assert!(error.contains("kebab-case"));
}

#[test]
fn only_one_default_sort_field_is_allowed() {
    let source = changed(
        &device_source(),
        "create = true\nupdate = true\nread = true\nlist = true\nfilter = true",
        "create = true\nupdate = true\nread = true\nlist = true\nfilter = true\nsort = true",
    );
    let error = normalize(&source, "device").expect_err("多个默认排序字段必须在 IR 阶段拒绝");
    assert!(error.contains("资源 device"));
    assert!(error.contains("字段 name"));
    assert!(error.contains("只能声明一个默认排序字段"));
    assert!(error.contains("catalog/resources/device.toml"));
    assert!(error.contains("只为一个字段保留 usage.sort=true"));
}

#[test]
fn schema_revision_uses_the_append_migration_name_format() {
    let source = changed(
        &device_source(),
        "bootstrap_migration = true",
        "bootstrap_migration = true\nschema_revision = \"device_v2\"",
    );
    let error = normalize(&source, "device").expect_err("任意 revision 名不得进入 ownership");
    assert!(error.contains("schema_revision `device_v2`"));
    assert!(error.contains("mYYYYMMDD_HHMMSS_name"));
}

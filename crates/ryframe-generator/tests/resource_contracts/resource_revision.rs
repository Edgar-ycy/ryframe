use std::path::Path;

use ryframe_generator::{
    ResourceSpec, WidgetSpec, load_resource, normalize_resource, render_resources,
};

const NOTICE_PATH: &str = "catalog/resources/notice.toml";
const NOTICE_SOURCE: &str = include_str!("../../../../catalog/resources/notice.toml");

fn notice() -> ResourceSpec {
    ResourceSpec::parse(NOTICE_SOURCE, NOTICE_PATH).unwrap()
}

#[test]
fn revision_is_a_read_value_and_required_update_guard_for_opted_in_resources() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let notice = load_resource(root.join(NOTICE_PATH)).unwrap();
    let post = load_resource(root.join("catalog/resources/post.toml")).unwrap();
    let notice_json = serde_json::to_value(&notice).unwrap();
    let post_json = serde_json::to_value(&post).unwrap();
    assert_eq!(notice_json["revision_field"], "revision");
    assert_eq!(
        notice_json["schema_revision"],
        "m20261120_072911_notice_revision"
    );
    assert!(post_json.get("revision_field").is_none());

    let rendered = render_resources(&[notice, post]).unwrap();
    let content = |suffix: &str| {
        rendered
            .assets
            .iter()
            .find(|asset| asset.path.ends_with(suffix))
            .unwrap()
            .content
            .as_str()
    };
    let dto = content("notice/dto.rs");
    let (create, after_create) = dto.split_once("impl From<CreateNoticeDto>").unwrap();
    assert!(!create.contains("expected_revision"));
    let (_, update) = after_create
        .split_once("#[derive(Debug, Deserialize, Validate, ToSchema)]")
        .unwrap();
    let (input, output) = update.split_once("pub struct NoticeVo").unwrap();
    assert!(input.contains(concat!(
        "#[schema(value_type = String, pattern = r\"^(?:0|[1-9][0-9]*)$\")]\n",
        "    #[serde(deserialize_with = \"crate::http::integer::deserialize_u64\")]\n",
        "    pub expected_revision: u64,",
    )));
    assert!(input.contains("expected_revision: value.expected_revision"));
    assert!(output.contains("pub revision: String,"));
    assert!(output.contains("revision: value.revision.to_string()"));

    let service = content("notice/service.rs");
    let (before, update) = service.split_once("pub async fn update(").unwrap();
    assert!(before.contains("revision: 1"));
    let locked = update.find(".find_by_id_for_update(").unwrap();
    let matched = update
        .find("record.revision != command.expected_revision")
        .unwrap();
    let advanced = update.find(".checked_add(1)").unwrap();
    let saved = update.find("transaction.update(record)").unwrap();
    assert!(locked < matched && matched < advanced && advanced < saved);
    assert!(update.contains("AppError::Conflict("));
    assert!(content("notice/handler.rs").contains("status = 409"));
    assert!(!content("post/handler.rs").contains("status = 409"));
    assert!(!content("post/dto.rs").contains("expected_revision"));
}

#[test]
fn revision_rejects_missing_owner_writable_fields_and_request_name_collisions() {
    let mut input = notice();
    input.access.owner_field = None;
    let error = normalize_resource(input, NOTICE_PATH, "notice-without-owner").unwrap_err();
    assert!(error.to_string().contains("租户和归属行锁"));

    let mut input = notice();
    let revision = input
        .fields
        .iter_mut()
        .find(|field| field.name == "revision")
        .unwrap();
    revision.usage.update = true;
    revision.widget = WidgetSpec::Text;
    let error = normalize_resource(input, NOTICE_PATH, "notice-with-writable-version").unwrap_err();
    assert!(error.to_string().contains("非空只读 u64"));

    let mut input = notice();
    let mut duplicate = input.fields.last().unwrap().clone();
    duplicate.name = "expected_revision".into();
    duplicate.order += 10;
    input.fields.push(duplicate);
    let error = normalize_resource(input, NOTICE_PATH, "notice-with-reserved-field").unwrap_err();
    assert!(error.to_string().contains("expected_revision 已保留"));
}

#[test]
fn revision_rejects_a_different_well_formed_field_name() {
    let mut input = notice();
    let revision = input
        .fields
        .iter_mut()
        .find(|field| field.name == "revision")
        .unwrap();
    revision.name = "row_version".into();
    input.database.revision_field = Some("row_version".into());

    let error =
        normalize_resource(input, NOTICE_PATH, "notice-with-alternate-version").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("database.revision_field 只支持 revision")
    );
}

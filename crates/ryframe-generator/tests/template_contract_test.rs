use ryframe_generator::{
    ColumnInfo, TableInfo,
    template::{dto, entity, handler, repository, service},
};

fn column(name: &str, rust_type: &str) -> ColumnInfo {
    ColumnInfo {
        name: name.into(),
        data_type: "test".into(),
        rust_type: rust_type.into(),
        is_nullable: rust_type.starts_with("Option<"),
        is_primary_key: name == "id",
        is_unique: name == "name",
        is_auto_increment: false,
        comment: Some(format!("{name} field")),
    }
}

fn sample_table() -> TableInfo {
    TableInfo {
        table_name: "sys_widget".into(),
        comment: Some("Widget table".into()),
        columns: vec![
            column("id", "i64"),
            column("tenant_id", "String"),
            column("name", "String"),
            column("status", "String"),
            column("del_flag", "String"),
            column("created_at", "DateTime<Utc>"),
            column("updated_at", "DateTime<Utc>"),
        ],
    }
}

fn rendered_templates() -> Vec<(&'static str, String)> {
    let table = sample_table();
    vec![
        ("entity", entity::render_entity(&table, "widget", true)),
        (
            "repository",
            repository::render_repository(&table, "widget"),
        ),
        ("dto", dto::render_dto(&table, "widget")),
        ("service", service::render_service(&table, "widget")),
        ("handler", handler::render_handler(&table, "widget")),
    ]
}

#[test]
fn generated_templates_are_valid_rust_syntax() {
    for (name, source) in rendered_templates() {
        syn::parse_file(&source).unwrap_or_else(|error| panic!("{name}: {error}\n{source}"));
    }
}

#[test]
fn generated_templates_follow_application_boundaries() {
    let templates = rendered_templates();
    let source = |name| {
        templates
            .iter()
            .find(|(template_name, _)| *template_name == name)
            .map(|(_, source)| source.as_str())
            .unwrap()
    };

    let handler = source("handler");
    assert!(handler.contains("use crate::state::AppState;"));
    assert!(handler.contains("use ryframe_auth::RequestPrincipal;"));
    assert!(handler.contains("current_user: RequestPrincipal"));
    assert!(handler.contains(".find_by_page(&current_user, page_query)"));
    assert!(handler.contains(".services\n        .widget"));
    assert!(!handler.contains("sea_orm"));
    assert!(!handler.contains("state.db"));
    assert!(!handler.contains("WidgetService::new"));

    let service = source("service");
    assert!(service.contains("pub struct WidgetService"));
    assert!(service.contains("widget_repo: LoggedRepo<WidgetRepository>"));
    assert!(service.contains("db: DatabaseConnection"));
    assert!(service.contains("pub fn new(db: DatabaseConnection)"));
    assert!(service.contains("actor: &ActorContext"));
    assert!(service.contains("let tenant_id = crate::validated_tenant_id(actor)?;"));
    assert!(service.contains(".find_by_id(db, tenant_id, id)"));
    assert!(!service.contains("db: &DatabaseConnection"));
    assert!(!service.contains("pub widget_repo"));
    assert!(!service.contains("Arc<DatabaseConnection>"));
    assert!(!service.contains("crate::dto"));
    assert!(service.contains("snowflake::try_next_snowflake_id()?"));
    assert!(service.contains("model.fill_on_insert(&FillContext::new())?"));
    assert!(!service.contains("snowflake::next_snowflake_id()"));

    let dto = source("dto");
    assert!(dto.contains("#[serde(deny_unknown_fields)]"));
    assert!(!dto.contains("pub id:"));
    assert!(!dto.contains("pub tenant_id:"));

    let repository = source("repository");
    assert!(repository.contains("Column::TenantId"));
    assert!(repository.contains("Column::DelFlag"));
    assert!(repository.contains("tenant_id: &str"));
    assert!(repository.contains("Column::TenantId.eq(tenant_id)"));
    assert!(!repository.contains("current_tenant_id"));
}

#[test]
fn generated_template_output_matches_golden_hashes() {
    let actual = rendered_templates()
        .into_iter()
        .map(|(name, source)| (name, fnv1a64(source.as_bytes())))
        .collect::<Vec<_>>();
    let expected = vec![
        ("entity", 15_143_636_760_214_386_781),
        ("repository", 13_192_807_906_955_434_705),
        ("dto", 8_322_372_481_273_473_967),
        ("service", 6_467_106_141_933_554_713),
        ("handler", 4_400_737_824_380_872_182),
    ];
    assert_eq!(actual, expected);
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

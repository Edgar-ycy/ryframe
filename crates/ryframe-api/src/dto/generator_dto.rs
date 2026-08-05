use ryframe_service::system::generator_service::GenerateOptions;
use serde::Deserialize;
use utoipa::ToSchema;

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GenerateOptionsDto {
    pub tables: Vec<String>,
    pub entity_dir: Option<String>,
    pub repository_dir: Option<String>,
    pub service_dir: Option<String>,
    pub handler_dir: Option<String>,
    pub dto_dir: Option<String>,
    pub generate_entity: Option<bool>,
    pub generate_repository: Option<bool>,
    pub generate_service: Option<bool>,
    pub generate_handler: Option<bool>,
    pub generate_dto: Option<bool>,
    #[serde(default)]
    pub table_prefixes: Vec<String>,
    #[serde(default)]
    pub generate_comments: bool,
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GenerateRequestDto {
    /// 后端服务所在机器上的代码输出根目录。
    pub output_dir: String,
    pub options: GenerateOptionsDto,
}

impl From<GenerateOptionsDto> for GenerateOptions {
    fn from(dto: GenerateOptionsDto) -> Self {
        let defaults = Self::default();
        Self {
            tables: dto.tables,
            entity_dir: dto.entity_dir.unwrap_or(defaults.entity_dir),
            repository_dir: dto.repository_dir.unwrap_or(defaults.repository_dir),
            service_dir: dto.service_dir.unwrap_or(defaults.service_dir),
            handler_dir: dto.handler_dir.unwrap_or(defaults.handler_dir),
            dto_dir: dto.dto_dir.unwrap_or(defaults.dto_dir),
            generate_entity: dto.generate_entity.unwrap_or(defaults.generate_entity),
            generate_repository: dto
                .generate_repository
                .unwrap_or(defaults.generate_repository),
            generate_service: dto.generate_service.unwrap_or(defaults.generate_service),
            generate_handler: dto.generate_handler.unwrap_or(defaults.generate_handler),
            generate_dto: dto.generate_dto.unwrap_or(defaults.generate_dto),
            table_prefixes: dto.table_prefixes,
            generate_comments: dto.generate_comments,
            overwrite: dto.overwrite,
        }
    }
}

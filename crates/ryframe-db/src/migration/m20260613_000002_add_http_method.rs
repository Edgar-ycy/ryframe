use sea_orm_migration::prelude::*;

/// 为 sys_permission 表添加 http_method 字段（补充 init_tables 迁移中的遗漏）
pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20260613_000002_add_http_method"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(SysPermission::Table)
                    .add_column_if_not_exists(
                        ColumnDef::new(SysPermission::HttpMethod)
                            .string_len(10)
                            .default(Expr::value("")),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(SysPermission::Table)
                    .drop_column(SysPermission::HttpMethod)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum SysPermission {
    Table,
    HttpMethod,
}

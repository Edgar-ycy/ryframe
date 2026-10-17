use std::collections::{BTreeMap, BTreeSet};

use sea_orm::{ConnectionTrait, DbBackend, DbErr, Statement};

use super::registered;

/// 仅核验已编入进程的业务目录，不修改角色授权、用户名称或菜单显示状态。
pub async fn verify_business_access_catalog<C: ConnectionTrait + ?Sized>(
    db: &C,
) -> Result<(), DbErr> {
    let Some(catalog) = registered() else {
        return Ok(());
    };
    if catalog.resources().next().is_none() {
        return Ok(());
    }
    let mut codes = BTreeSet::new();
    for batch in catalog
        .permission_names()
        .keys()
        .collect::<Vec<_>>()
        .chunks(256)
    {
        let sql = format!(
            "SELECT code FROM sys_permission WHERE tenant_id='system' AND code IN ({})",
            placeholders(batch.len())
        );
        for row in db
            .query_all_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                sql,
                batch
                    .iter()
                    .map(|code| code.as_str().into())
                    .collect::<Vec<sea_orm::Value>>(),
            ))
            .await?
        {
            codes.insert(row.try_get::<String>("", "code")?);
        }
    }
    for code in catalog.permission_names().keys() {
        if !codes.contains(code) {
            return Err(missing(format!("业务权限 {code} 尚未初始化")));
        }
    }
    let mut menus = BTreeMap::new();
    for batch in catalog.menus().chunks(256) {
        let sql = format!(
            "SELECT m.route_key,m.menu_type,p.code AS permission_code,parent.route_key AS parent_route_key FROM sys_menu m LEFT JOIN sys_permission p ON p.id=m.perm_id AND p.tenant_id=m.tenant_id LEFT JOIN sys_menu parent ON parent.id=m.parent_id AND parent.tenant_id=m.tenant_id AND parent.del_flag='0' WHERE m.tenant_id='system' AND m.del_flag='0' AND m.route_key IN ({})",
            placeholders(batch.len())
        );
        for row in db
            .query_all_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                sql,
                batch
                    .iter()
                    .map(|menu| menu.route_key.as_str().into())
                    .collect::<Vec<sea_orm::Value>>(),
            ))
            .await?
        {
            let key = row.try_get::<String>("", "route_key")?;
            let value = (
                row.try_get::<String>("", "menu_type")?,
                row.try_get::<Option<String>>("", "permission_code")?,
                row.try_get::<Option<String>>("", "parent_route_key")?,
            );
            if menus.insert(key, value).is_some() {
                return Err(missing("业务菜单存在重复路由键".into()));
            }
        }
    }
    for menu in catalog.menus() {
        if menus
            .get(&menu.route_key)
            .is_none_or(|(kind, permission, parent)| {
                kind != &menu.menu_type
                    || permission != &menu.permission
                    || parent.as_deref() != menu.parent_route_key()
            })
        {
            return Err(missing(format!(
                "业务菜单 {} 缺失、已删除、父目录或权限绑定不一致",
                menu.route_key
            )));
        }
    }
    Ok(())
}

fn placeholders(count: usize) -> String {
    vec!["?"; count].join(",")
}

fn missing(reason: String) -> DbErr {
    DbErr::Custom(format!(
        "业务访问目录校验失败：{reason}；不会自动修复。先运行 cargo migrate -- control status 核对当前库和已有写入，审阅后执行 control up，再执行 control verify"
    ))
}

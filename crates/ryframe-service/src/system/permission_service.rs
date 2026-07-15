use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use ryframe_common::{AppError, AppResult, utils::snowflake};
use ryframe_core::{
    LoggedRepo, RedisClient, Repository,
    auto_fill::{AutoFill, FillContext},
    cache::clear_tenant_permission_cache,
};
use ryframe_db::{PermissionRepository, entities::permission};
use sea_orm::DatabaseConnection;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct PermissionTreeNode {
    /// id 使用 String 避免 Snowflake 64 位 ID 超出 JS Number.MAX_SAFE_INTEGER
    pub id: String,
    pub name: String,
    pub code: String,
    pub parent_id: Option<String>,
    pub perm_type: String,
    pub icon: Option<String>,
    pub sort: i32,
    pub status: String,
    pub children: Vec<PermissionTreeNode>,
}

pub struct PermissionServiceImpl {
    pub perm_repo: LoggedRepo<PermissionRepository>,
    pub redis: Option<RedisClient>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PermissionSyncReport {
    pub scanned: usize,
    pub existing: usize,
    pub created: usize,
    pub missing: Vec<String>,
}

impl PermissionServiceImpl {
    async fn invalidate_permission_cache(&self) {
        if let Some(redis) = &self.redis
            && let Err(error) =
                clear_tenant_permission_cache(redis, &ryframe_core::current_tenant_id()).await
        {
            tracing::warn!(%error, "failed to clear tenant permission cache");
        }
    }

    fn default_code_source_roots() -> Vec<PathBuf> {
        let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        vec![
            workspace_root.join("crates/ryframe-api/src/handlers"),
            workspace_root.join("crates/ryframe-api/src/router.rs"),
            workspace_root.join("crates/ryframe-monitor/src/lib.rs"),
            workspace_root.join("crates/ryframe-api/src/openapi.rs"),
        ]
    }

    fn extract_permission_codes_from_text(text: &str) -> Vec<String> {
        let mut codes = Vec::new();
        let needle = "#[perm(\"";
        let mut idx = 0;
        while let Some(start) = text[idx..].find(needle) {
            let start = idx + start + needle.len();
            let Some(end_offset) = text[start..].find('"') else {
                break;
            };
            let code = &text[start..start + end_offset];
            if code.contains(':') || code.contains('*') {
                codes.push(code.to_string());
            }
            idx = start + end_offset + 1;
        }
        codes
    }

    pub fn scan_permission_codes() -> AppResult<Vec<String>> {
        let mut codes = HashSet::new();
        for root in Self::default_code_source_roots() {
            if root.is_file() {
                if let Ok(text) = fs::read_to_string(&root) {
                    for code in Self::extract_permission_codes_from_text(&text) {
                        codes.insert(code);
                    }
                }
                continue;
            }

            if !root.exists() {
                continue;
            }

            Self::collect_codes_from_path(&root, &mut codes)?;
        }

        let mut list: Vec<String> = codes.into_iter().collect();
        list.sort();
        Ok(list)
    }

    fn collect_codes_from_path(path: &Path, codes: &mut HashSet<String>) -> AppResult<()> {
        if path.is_file() {
            if path.extension().and_then(|s| s.to_str()) == Some("rs") {
                let text =
                    fs::read_to_string(path).map_err(|e| AppError::Internal(e.to_string()))?;
                for code in Self::extract_permission_codes_from_text(&text) {
                    codes.insert(code);
                }
            }
            return Ok(());
        }

        if !path.is_dir() {
            return Ok(());
        }

        for entry in fs::read_dir(path).map_err(|e| AppError::Internal(e.to_string()))? {
            let entry = entry.map_err(|e| AppError::Internal(e.to_string()))?;
            Self::collect_codes_from_path(&entry.path(), codes)?;
        }
        Ok(())
    }

    pub async fn list_all_perms(
        &self,
        db: &DatabaseConnection,
        perm_type: Option<&str>,
    ) -> AppResult<Vec<PermissionTreeNode>> {
        let all = self.perm_repo.find_all(db).await?;
        let filtered: Vec<&permission::Model> = if let Some(t) = perm_type {
            all.iter().filter(|p| p.perm_type == t).collect()
        } else {
            all.iter().collect()
        };

        let models: Vec<&permission::Model> = filtered;
        Ok(build_perm_tree(&models, None))
    }

    pub async fn find_by_id(
        &self,
        db: &DatabaseConnection,
        id: i64,
    ) -> AppResult<Option<permission::Model>> {
        self.perm_repo.find_by_id(db, id).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        db: &DatabaseConnection,
        name: &str,
        code: &str,
        parent_id: Option<i64>,
        perm_type: &str,
        icon: Option<&str>,
        sort: i32,
        status: &str,
    ) -> AppResult<permission::Model> {
        if self.perm_repo.find_by_code(db, code).await?.is_some() {
            return Err(AppError::Conflict("权限码已存在".into()));
        }
        let mut model = permission::Model {
            id: snowflake::next_snowflake_id(),
            tenant_id: ryframe_core::current_tenant_id(),
            name: name.to_string(),
            code: code.to_string(),
            parent_id,
            perm_type: perm_type.to_string(),
            icon: icon.map(|v| v.to_string()),
            sort,
            status: status.to_string(),
            created_at: Default::default(),
            updated_at: Default::default(),
        };
        model.fill_on_insert(&FillContext::new());
        let saved = self.perm_repo.insert(db, model).await?;
        self.invalidate_permission_cache().await;
        Ok(saved)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update(
        &self,
        db: &DatabaseConnection,
        id: i64,
        name: &str,
        code: &str,
        parent_id: Option<i64>,
        perm_type: &str,
        icon: Option<&str>,
        sort: i32,
        status: &str,
    ) -> AppResult<permission::Model> {
        let mut model = self
            .perm_repo
            .find_by_id(db, id)
            .await?
            .ok_or_else(|| AppError::NotFound("权限不存在".into()))?;
        if model.code != code && self.perm_repo.find_by_code(db, code).await?.is_some() {
            return Err(AppError::Conflict("权限码已存在".into()));
        }
        model.name = name.to_string();
        model.code = code.to_string();
        model.parent_id = parent_id;
        model.perm_type = perm_type.to_string();
        model.icon = icon.map(|v| v.to_string());
        model.sort = sort;
        model.status = status.to_string();
        model.fill_on_update(&FillContext::new());
        use sea_orm::{ActiveModelTrait, ActiveValue};
        let active = permission::ActiveModel {
            id: ActiveValue::Unchanged(model.id),
            tenant_id: ActiveValue::Set(model.tenant_id.clone()),
            name: ActiveValue::Set(model.name.clone()),
            code: ActiveValue::Set(model.code.clone()),
            parent_id: ActiveValue::Set(model.parent_id),
            perm_type: ActiveValue::Set(model.perm_type.clone()),
            icon: ActiveValue::Set(model.icon.clone()),
            sort: ActiveValue::Set(model.sort),
            status: ActiveValue::Set(model.status.clone()),
            created_at: ActiveValue::Set(model.created_at),
            updated_at: ActiveValue::Set(model.updated_at),
        };
        let saved = active
            .update(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        self.invalidate_permission_cache().await;
        Ok(saved)
    }

    pub async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        if self.perm_repo.is_referenced(db, id).await? {
            return Err(AppError::Conflict(
                "权限仍被角色或菜单引用，不能删除".into(),
            ));
        }
        self.perm_repo.delete(db, id).await?;
        self.invalidate_permission_cache().await;
        Ok(())
    }

    pub async fn sync_perm_from_route(
        &self,
        db: &DatabaseConnection,
    ) -> AppResult<PermissionSyncReport> {
        let scanned = Self::scan_permission_codes()?;
        let existing = self.perm_repo.find_all(db).await?;
        let existing_codes: HashSet<String> = existing.iter().map(|p| p.code.clone()).collect();
        let scanned_total = scanned.len();
        let mut created = 0usize;
        let mut missing = Vec::new();

        for code in scanned {
            if existing_codes.contains(&code) {
                continue;
            }
            missing.push(code.clone());
            let name = code.rsplit(':').next().unwrap_or(&code).to_string();
            let mut model = permission::Model {
                id: snowflake::next_snowflake_id(),
                tenant_id: ryframe_core::current_tenant_id(),
                name,
                code: code.clone(),
                parent_id: None,
                perm_type: "api".to_string(),
                icon: None,
                sort: 0,
                status: "1".to_string(),
                created_at: Default::default(),
                updated_at: Default::default(),
            };
            model.fill_on_insert(&FillContext::new());
            self.perm_repo.insert(db, model).await?;
            created += 1;
        }

        if created > 0 {
            self.invalidate_permission_cache().await;
        }

        Ok(PermissionSyncReport {
            scanned: scanned_total,
            existing: existing_codes.len(),
            created,
            missing,
        })
    }
}

pub fn build_perm_tree(
    perms: &[&permission::Model],
    parent_id: Option<i64>,
) -> Vec<PermissionTreeNode> {
    perms
        .iter()
        .filter(|p| p.parent_id == parent_id)
        .map(|p| PermissionTreeNode {
            id: p.id.to_string(),
            name: p.name.clone(),
            code: p.code.clone(),
            parent_id: p.parent_id.map(|p| p.to_string()),
            perm_type: p.perm_type.clone(),
            icon: p.icon.clone(),
            sort: p.sort,
            status: p.status.clone(),
            children: build_perm_tree(perms, Some(p.id)),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::PermissionServiceImpl;

    #[test]
    fn extracts_attribute_permission_codes() {
        let source = r#"
            #[get("/list")]
            #[perm("system:user:list")]
            async fn list() {}
        "#;

        assert_eq!(
            PermissionServiceImpl::extract_permission_codes_from_text(source),
            vec![String::from("system:user:list")]
        );
    }
}

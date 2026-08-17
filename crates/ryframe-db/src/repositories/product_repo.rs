use ryframe_kernel::{AppError, AppResult};
use sea_orm::{
    ActiveModelTrait,
    ActiveValue::Set,
    ColumnTrait, ConnectionTrait, DatabaseConnection, DatabaseTransaction, EntityTrait, ExprTrait,
    IntoActiveModel, QueryFilter, QueryOrder, QuerySelect,
    sea_query::{Expr, LockType},
};

use crate::entities::{
    product_plan, product_plan_capability, product_plan_version, tenant,
    tenant_capability_override, tenant_product_plan,
};

#[derive(Clone, Debug)]
pub struct ProductPlanVersionBundle {
    pub plan: product_plan::Model,
    pub version: product_plan_version::Model,
    pub capabilities: Vec<product_plan_capability::Model>,
}

#[derive(Clone, Debug)]
pub struct TenantProductBundle {
    pub tenant: tenant::Model,
    pub assignment: tenant_product_plan::Model,
    pub plan: product_plan::Model,
    pub version: product_plan_version::Model,
    pub capabilities: Vec<product_plan_capability::Model>,
    pub overrides: Vec<tenant_capability_override::Model>,
}

pub struct ProductRepository;

impl ProductRepository {
    pub async fn list_plans(&self, db: &DatabaseConnection) -> AppResult<Vec<product_plan::Model>> {
        product_plan::Entity::find()
            .order_by_asc(product_plan::Column::PlanKey)
            .all(db)
            .await
            .map_err(database_error)
    }

    pub async fn list_versions<C>(
        &self,
        db: &C,
        plan_id: i64,
    ) -> AppResult<Vec<product_plan_version::Model>>
    where
        C: ConnectionTrait,
    {
        product_plan_version::Entity::find()
            .filter(product_plan_version::Column::PlanId.eq(plan_id))
            .order_by_desc(product_plan_version::Column::Version)
            .all(db)
            .await
            .map_err(database_error)
    }

    pub async fn list_capabilities<C>(
        &self,
        db: &C,
        plan_version_id: i64,
    ) -> AppResult<Vec<product_plan_capability::Model>>
    where
        C: ConnectionTrait,
    {
        product_plan_capability::Entity::find()
            .filter(product_plan_capability::Column::PlanVersionId.eq(plan_version_id))
            .order_by_asc(product_plan_capability::Column::CapabilityCode)
            .all(db)
            .await
            .map_err(database_error)
    }

    pub async fn find_plan_by_key<C>(
        &self,
        db: &C,
        plan_key: &str,
    ) -> AppResult<Option<product_plan::Model>>
    where
        C: ConnectionTrait,
    {
        product_plan::Entity::find()
            .filter(product_plan::Column::PlanKey.eq(plan_key))
            .one(db)
            .await
            .map_err(database_error)
    }

    pub async fn find_plan_by_id<C>(
        &self,
        db: &C,
        plan_id: i64,
    ) -> AppResult<Option<product_plan::Model>>
    where
        C: ConnectionTrait,
    {
        product_plan::Entity::find_by_id(plan_id)
            .one(db)
            .await
            .map_err(database_error)
    }

    pub async fn find_version_by_id<C>(
        &self,
        db: &C,
        version_id: i64,
    ) -> AppResult<Option<ProductPlanVersionBundle>>
    where
        C: ConnectionTrait,
    {
        let Some(version) = product_plan_version::Entity::find_by_id(version_id)
            .one(db)
            .await
            .map_err(database_error)?
        else {
            return Ok(None);
        };
        let plan = product_plan::Entity::find_by_id(version.plan_id)
            .one(db)
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::Database("产品套餐版本引用了不存在的套餐".into()))?;
        let capabilities = self.list_capabilities(db, version.id).await?;
        Ok(Some(ProductPlanVersionBundle {
            plan,
            version,
            capabilities,
        }))
    }

    pub async fn find_version<C>(
        &self,
        db: &C,
        plan_key: &str,
        version: i32,
    ) -> AppResult<Option<ProductPlanVersionBundle>>
    where
        C: ConnectionTrait,
    {
        let Some(plan) = self.find_plan_by_key(db, plan_key).await? else {
            return Ok(None);
        };
        let Some(version) = product_plan_version::Entity::find()
            .filter(product_plan_version::Column::PlanId.eq(plan.id))
            .filter(product_plan_version::Column::Version.eq(version))
            .one(db)
            .await
            .map_err(database_error)?
        else {
            return Ok(None);
        };
        let capabilities = self.list_capabilities(db, version.id).await?;
        Ok(Some(ProductPlanVersionBundle {
            plan,
            version,
            capabilities,
        }))
    }

    pub async fn tenant_product<C>(
        &self,
        db: &C,
        tenant_id: &str,
    ) -> AppResult<Option<TenantProductBundle>>
    where
        C: ConnectionTrait,
    {
        let Some(tenant) = tenant::Entity::find()
            .filter(tenant::Column::TenantId.eq(tenant_id))
            .one(db)
            .await
            .map_err(database_error)?
        else {
            return Ok(None);
        };
        let assignment = tenant_product_plan::Entity::find_by_id(tenant_id.to_owned())
            .one(db)
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::Database("租户缺少产品套餐分配".into()))?;
        let version = product_plan_version::Entity::find_by_id(assignment.plan_version_id)
            .one(db)
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::Database("租户引用了不存在的产品套餐版本".into()))?;
        let plan = product_plan::Entity::find_by_id(version.plan_id)
            .one(db)
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::Database("产品套餐版本引用了不存在的套餐".into()))?;
        let capabilities = self.list_capabilities(db, version.id).await?;
        let overrides = tenant_capability_override::Entity::find()
            .filter(tenant_capability_override::Column::TenantId.eq(tenant_id))
            .order_by_asc(tenant_capability_override::Column::CapabilityCode)
            .all(db)
            .await
            .map_err(database_error)?;
        Ok(Some(TenantProductBundle {
            tenant,
            assignment,
            plan,
            version,
            capabilities,
            overrides,
        }))
    }

    /// 固定锁序中的资源阶段：调用前必须已锁定 tenant 与 operation lease。
    pub async fn lock_assignment_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
    ) -> AppResult<tenant_product_plan::Model> {
        tenant_product_plan::Entity::find_by_id(tenant_id.to_owned())
            .lock(LockType::Update)
            .one(transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::Database("租户缺少产品套餐分配".into()))
    }

    pub async fn assign_initial_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        plan_version_id: i64,
        changed_by: i64,
    ) -> AppResult<()> {
        let now = chrono::Utc::now();
        tenant_product_plan::ActiveModel {
            tenant_id: Set(tenant_id.to_owned()),
            plan_version_id: Set(plan_version_id),
            changed_by: Set(Some(changed_by)),
            change_reason: Set(Some("tenant_creation".into())),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(transaction)
        .await
        .map(|_| ())
        .map_err(database_error)
    }

    pub async fn assignment<C>(
        &self,
        db: &C,
        tenant_id: &str,
    ) -> AppResult<Option<tenant_product_plan::Model>>
    where
        C: ConnectionTrait,
    {
        tenant_product_plan::Entity::find_by_id(tenant_id.to_owned())
            .one(db)
            .await
            .map_err(database_error)
    }

    pub async fn replace_assignment_and_overrides_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        assignment: tenant_product_plan::Model,
        overrides: Vec<tenant_capability_override::Model>,
    ) -> AppResult<()> {
        let tenant_id = assignment.tenant_id.clone();
        assignment
            .into_active_model()
            .reset_all()
            .update(transaction)
            .await
            .map_err(database_error)?;
        tenant_capability_override::Entity::delete_many()
            .filter(tenant_capability_override::Column::TenantId.eq(&tenant_id))
            .exec(transaction)
            .await
            .map_err(database_error)?;
        if !overrides.is_empty() {
            tenant_capability_override::Entity::insert_many(
                overrides
                    .into_iter()
                    .map(tenant_capability_override::ActiveModel::from),
            )
            .exec(transaction)
            .await
            .map_err(database_error)?;
        }
        Ok(())
    }

    pub async fn increment_runtime_epoch_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        expected_epoch: i64,
    ) -> AppResult<i64> {
        let result = tenant::Entity::update_many()
            .col_expr(
                tenant::Column::RuntimeEpoch,
                Expr::col(tenant::Column::RuntimeEpoch).add(1),
            )
            .col_expr(tenant::Column::UpdatedAt, Expr::cust("UTC_TIMESTAMP(6)"))
            .filter(tenant::Column::TenantId.eq(tenant_id))
            .filter(tenant::Column::RuntimeEpoch.eq(expected_epoch))
            .exec(transaction)
            .await
            .map_err(database_error)?;
        if result.rows_affected != 1 {
            return Err(AppError::StaleRuntimeEpoch(
                "租户运行时上下文已变化，请重新预览产品变更".into(),
            ));
        }
        Ok(expected_epoch + 1)
    }

    pub async fn insert_plan_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        plan: product_plan::Model,
    ) -> AppResult<product_plan::Model> {
        product_plan::ActiveModel::from(plan)
            .insert(transaction)
            .await
            .map_err(database_error)
    }

    pub async fn update_plan_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        plan: product_plan::Model,
    ) -> AppResult<product_plan::Model> {
        plan.into_active_model()
            .reset_all()
            .update(transaction)
            .await
            .map_err(database_error)
    }

    pub async fn lock_plan_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        plan_key: &str,
    ) -> AppResult<product_plan::Model> {
        product_plan::Entity::find()
            .filter(product_plan::Column::PlanKey.eq(plan_key))
            .lock(LockType::Update)
            .one(transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::NotFound("产品套餐不存在".into()))
    }

    pub async fn lock_plan_by_id_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        plan_id: i64,
    ) -> AppResult<product_plan::Model> {
        product_plan::Entity::find_by_id(plan_id)
            .lock(LockType::Update)
            .one(transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::NotFound("产品套餐不存在".into()))
    }

    pub async fn next_version_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        plan_id: i64,
    ) -> AppResult<i32> {
        let current = product_plan_version::Entity::find()
            .filter(product_plan_version::Column::PlanId.eq(plan_id))
            .order_by_desc(product_plan_version::Column::Version)
            .select_only()
            .column(product_plan_version::Column::Version)
            .into_tuple::<i32>()
            .one(transaction)
            .await
            .map_err(database_error)?
            .unwrap_or(0);
        current
            .checked_add(1)
            .ok_or_else(|| AppError::Conflict("产品套餐版本号已耗尽".into()))
    }

    pub async fn insert_version_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        version: product_plan_version::Model,
        capabilities: Vec<product_plan_capability::Model>,
    ) -> AppResult<product_plan_version::Model> {
        let saved = product_plan_version::ActiveModel::from(version)
            .insert(transaction)
            .await
            .map_err(database_error)?;
        if !capabilities.is_empty() {
            product_plan_capability::Entity::insert_many(
                capabilities
                    .into_iter()
                    .map(product_plan_capability::ActiveModel::from),
            )
            .exec(transaction)
            .await
            .map_err(database_error)?;
        }
        Ok(saved)
    }

    pub async fn lock_version_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        plan_id: i64,
        version: i32,
    ) -> AppResult<product_plan_version::Model> {
        product_plan_version::Entity::find()
            .filter(product_plan_version::Column::PlanId.eq(plan_id))
            .filter(product_plan_version::Column::Version.eq(version))
            .lock(LockType::Update)
            .one(transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::NotFound("产品套餐版本不存在".into()))
    }

    pub async fn lock_version_by_id_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        version_id: i64,
    ) -> AppResult<product_plan_version::Model> {
        product_plan_version::Entity::find_by_id(version_id)
            .lock(LockType::Update)
            .one(transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::NotFound("产品套餐版本不存在".into()))
    }

    pub async fn replace_draft_version_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        version: product_plan_version::Model,
        capabilities: Vec<product_plan_capability::Model>,
    ) -> AppResult<product_plan_version::Model> {
        let version_id = version.id;
        let current = product_plan_version::Entity::find_by_id(version_id)
            .lock(LockType::Update)
            .one(transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::NotFound("产品套餐版本不存在".into()))?;
        if current.status != "draft" || version.status != "draft" {
            return Err(AppError::Conflict(
                "只有 draft 产品套餐版本可以修改内容".into(),
            ));
        }
        let saved = version
            .into_active_model()
            .reset_all()
            .update(transaction)
            .await
            .map_err(database_error)?;
        product_plan_capability::Entity::delete_many()
            .filter(product_plan_capability::Column::PlanVersionId.eq(version_id))
            .exec(transaction)
            .await
            .map_err(database_error)?;
        if !capabilities.is_empty() {
            product_plan_capability::Entity::insert_many(
                capabilities
                    .into_iter()
                    .map(product_plan_capability::ActiveModel::from),
            )
            .exec(transaction)
            .await
            .map_err(database_error)?;
        }
        Ok(saved)
    }

    pub async fn transition_version_status_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        version: product_plan_version::Model,
        expected_status: &str,
        target_status: &str,
    ) -> AppResult<product_plan_version::Model> {
        let current = product_plan_version::Entity::find_by_id(version.id)
            .lock(LockType::Update)
            .one(transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::NotFound("产品套餐版本不存在".into()))?;
        if current.status != expected_status || version.status != target_status {
            return Err(AppError::Conflict(format!(
                "产品套餐版本状态已变化，期望 {expected_status}"
            )));
        }
        version
            .into_active_model()
            .reset_all()
            .update(transaction)
            .await
            .map_err(database_error)
    }
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}

use ryframe_kernel::{AppError, AppResult};
use sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder,
    QuerySelect,
};

use crate::entities::{dept, dict_data, dict_type, post, user};

/// 已在服务层求交集后的最终行范围；服务账号的 SelfOnly 必须转换为 Empty。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentRowScope {
    All,
    Departments(Vec<i64>),
    DepartmentsAndUser {
        department_ids: Vec<i64>,
        user_id: i64,
    },
    User(i64),
    Empty,
}

#[derive(Clone, Debug)]
pub struct AgentQueryPage<T> {
    pub records: Vec<T>,
    pub total: u64,
}

#[derive(Clone, Debug)]
pub struct AgentDictionaryPage {
    pub dict_type: dict_type::Model,
    pub records: Vec<dict_data::Model>,
    pub total: u64,
}

pub struct AgentQueryRepository;

impl AgentQueryRepository {
    pub async fn users_page<C>(
        &self,
        db: &C,
        tenant_id: &str,
        scope: &AgentRowScope,
        offset: u64,
        limit: u64,
    ) -> AppResult<AgentQueryPage<user::Model>>
    where
        C: ConnectionTrait,
    {
        if limit == 0 || matches!(scope, AgentRowScope::Empty) {
            return Ok(empty_page());
        }
        let mut select = user::Entity::find()
            .filter(user::Column::TenantId.eq(tenant_id))
            .filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL));
        select = match scope {
            AgentRowScope::All => select,
            AgentRowScope::Departments(ids) if ids.is_empty() => return Ok(empty_page()),
            AgentRowScope::Departments(ids) => {
                select.filter(user::Column::DeptId.is_in(ids.clone()))
            }
            AgentRowScope::DepartmentsAndUser {
                department_ids,
                user_id,
            } => select.filter(
                Condition::any()
                    .add(user::Column::DeptId.is_in(department_ids.clone()))
                    .add(user::Column::Id.eq(*user_id)),
            ),
            AgentRowScope::User(user_id) => select.filter(user::Column::Id.eq(*user_id)),
            AgentRowScope::Empty => unreachable!(),
        };
        let total = select.clone().count(db).await.map_err(database_error)?;
        let records = select
            .order_by_asc(user::Column::Id)
            .offset(offset)
            .limit(limit)
            .all(db)
            .await
            .map_err(database_error)?;
        Ok(AgentQueryPage { records, total })
    }

    pub async fn users<C>(
        &self,
        db: &C,
        tenant_id: &str,
        scope: &AgentRowScope,
        after_id: Option<i64>,
        limit: u64,
    ) -> AppResult<Vec<user::Model>>
    where
        C: ConnectionTrait,
    {
        if limit == 0 || matches!(scope, AgentRowScope::Empty) {
            return Ok(Vec::new());
        }
        let mut select = user::Entity::find()
            .filter(user::Column::TenantId.eq(tenant_id))
            .filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL));
        select = match scope {
            AgentRowScope::All => select,
            AgentRowScope::Departments(ids) if ids.is_empty() => return Ok(Vec::new()),
            AgentRowScope::Departments(ids) => {
                select.filter(user::Column::DeptId.is_in(ids.clone()))
            }
            AgentRowScope::DepartmentsAndUser {
                department_ids,
                user_id,
            } => select.filter(
                Condition::any()
                    .add(user::Column::DeptId.is_in(department_ids.clone()))
                    .add(user::Column::Id.eq(*user_id)),
            ),
            AgentRowScope::User(user_id) => select.filter(user::Column::Id.eq(*user_id)),
            AgentRowScope::Empty => unreachable!(),
        };
        if let Some(after_id) = after_id {
            select = select.filter(user::Column::Id.gt(after_id));
        }
        select
            .order_by_asc(user::Column::Id)
            .limit(limit)
            .all(db)
            .await
            .map_err(database_error)
    }

    pub async fn departments<C>(
        &self,
        db: &C,
        tenant_id: &str,
        scope: &AgentRowScope,
        after_id: Option<i64>,
        limit: u64,
    ) -> AppResult<Vec<dept::Model>>
    where
        C: ConnectionTrait,
    {
        if limit == 0 || matches!(scope, AgentRowScope::Empty | AgentRowScope::User(_)) {
            return Ok(Vec::new());
        }
        let mut select = dept::Entity::find()
            .filter(dept::Column::TenantId.eq(tenant_id))
            .filter(dept::Column::DelFlag.eq(dept::Model::DEL_FLAG_NORMAL));
        select = match scope {
            AgentRowScope::All => select,
            AgentRowScope::Departments(ids) if ids.is_empty() => return Ok(Vec::new()),
            AgentRowScope::Departments(ids) => select.filter(dept::Column::Id.is_in(ids.clone())),
            AgentRowScope::DepartmentsAndUser { department_ids, .. }
                if department_ids.is_empty() =>
            {
                return Ok(Vec::new());
            }
            AgentRowScope::DepartmentsAndUser { department_ids, .. } => {
                select.filter(dept::Column::Id.is_in(department_ids.clone()))
            }
            AgentRowScope::User(_) | AgentRowScope::Empty => unreachable!(),
        };
        if let Some(after_id) = after_id {
            select = select.filter(dept::Column::Id.gt(after_id));
        }
        select
            .order_by_asc(dept::Column::Id)
            .limit(limit)
            .all(db)
            .await
            .map_err(database_error)
    }

    pub async fn departments_page<C>(
        &self,
        db: &C,
        tenant_id: &str,
        scope: &AgentRowScope,
        offset: u64,
        limit: u64,
    ) -> AppResult<AgentQueryPage<dept::Model>>
    where
        C: ConnectionTrait,
    {
        if limit == 0 || matches!(scope, AgentRowScope::Empty | AgentRowScope::User(_)) {
            return Ok(empty_page());
        }
        let mut select = dept::Entity::find()
            .filter(dept::Column::TenantId.eq(tenant_id))
            .filter(dept::Column::DelFlag.eq(dept::Model::DEL_FLAG_NORMAL));
        select = match scope {
            AgentRowScope::All => select,
            AgentRowScope::Departments(ids) if ids.is_empty() => return Ok(empty_page()),
            AgentRowScope::Departments(ids) => select.filter(dept::Column::Id.is_in(ids.clone())),
            AgentRowScope::DepartmentsAndUser { department_ids, .. }
                if department_ids.is_empty() =>
            {
                return Ok(empty_page());
            }
            AgentRowScope::DepartmentsAndUser { department_ids, .. } => {
                select.filter(dept::Column::Id.is_in(department_ids.clone()))
            }
            AgentRowScope::User(_) | AgentRowScope::Empty => unreachable!(),
        };
        let total = select.clone().count(db).await.map_err(database_error)?;
        let records = select
            .order_by_asc(dept::Column::Id)
            .offset(offset)
            .limit(limit)
            .all(db)
            .await
            .map_err(database_error)?;
        Ok(AgentQueryPage { records, total })
    }

    pub async fn posts<C>(
        &self,
        db: &C,
        tenant_id: &str,
        after_id: Option<i64>,
        limit: u64,
    ) -> AppResult<Vec<post::Model>>
    where
        C: ConnectionTrait,
    {
        let mut select = post::Entity::find()
            .filter(post::Column::TenantId.eq(tenant_id))
            .filter(post::Column::Status.eq(post::Model::STATUS_NORMAL))
            .filter(post::Column::DelFlag.eq(post::Model::DEL_FLAG_NORMAL));
        if let Some(after_id) = after_id {
            select = select.filter(post::Column::Id.gt(after_id));
        }
        select
            .order_by_asc(post::Column::Id)
            .limit(limit)
            .all(db)
            .await
            .map_err(database_error)
    }

    pub async fn posts_page<C>(
        &self,
        db: &C,
        tenant_id: &str,
        offset: u64,
        limit: u64,
    ) -> AppResult<AgentQueryPage<post::Model>>
    where
        C: ConnectionTrait,
    {
        let select = post::Entity::find()
            .filter(post::Column::TenantId.eq(tenant_id))
            .filter(post::Column::Status.eq(post::Model::STATUS_NORMAL))
            .filter(post::Column::DelFlag.eq(post::Model::DEL_FLAG_NORMAL));
        let total = select.clone().count(db).await.map_err(database_error)?;
        let records = select
            .order_by_asc(post::Column::Id)
            .offset(offset)
            .limit(limit)
            .all(db)
            .await
            .map_err(database_error)?;
        Ok(AgentQueryPage { records, total })
    }

    pub async fn dictionaries<C>(
        &self,
        db: &C,
        tenant_id: &str,
        after_id: Option<i64>,
        limit: u64,
    ) -> AppResult<(Vec<dict_type::Model>, Vec<dict_data::Model>)>
    where
        C: ConnectionTrait,
    {
        let mut type_select = dict_type::Entity::find()
            .filter(dict_type::Column::TenantId.eq(tenant_id))
            .filter(dict_type::Column::DelFlag.eq(dict_type::Model::DEL_FLAG_NORMAL));
        if let Some(after_id) = after_id {
            type_select = type_select.filter(dict_type::Column::Id.gt(after_id));
        }
        let types = type_select
            .order_by_asc(dict_type::Column::Id)
            .limit(limit)
            .all(db)
            .await
            .map_err(database_error)?;
        let codes = types
            .iter()
            .map(|item| item.code.clone())
            .collect::<Vec<_>>();
        let data = if codes.is_empty() {
            Vec::new()
        } else {
            dict_data::Entity::find()
                .filter(dict_data::Column::TenantId.eq(tenant_id))
                .filter(dict_data::Column::TypeCode.is_in(codes))
                .filter(dict_data::Column::DelFlag.eq(dict_data::Model::DEL_FLAG_NORMAL))
                .order_by_asc(dict_data::Column::Id)
                .limit(limit)
                .all(db)
                .await
                .map_err(database_error)?
        };
        Ok((types, data))
    }

    /// 查询单个已启用字典类型及其有界、启用的数据行。
    pub async fn dictionary_by_type_code<C>(
        &self,
        db: &C,
        tenant_id: &str,
        type_code: &str,
        after_id: Option<i64>,
        limit: u64,
    ) -> AppResult<Option<(dict_type::Model, Vec<dict_data::Model>)>>
    where
        C: ConnectionTrait,
    {
        let Some(dict_type) = dict_type::Entity::find()
            .filter(dict_type::Column::TenantId.eq(tenant_id))
            .filter(dict_type::Column::Code.eq(type_code))
            .filter(dict_type::Column::Status.eq(dict_type::Model::STATUS_NORMAL))
            .filter(dict_type::Column::DelFlag.eq(dict_type::Model::DEL_FLAG_NORMAL))
            .one(db)
            .await
            .map_err(database_error)?
        else {
            return Ok(None);
        };
        let mut select = dict_data::Entity::find()
            .filter(dict_data::Column::TenantId.eq(tenant_id))
            .filter(dict_data::Column::TypeCode.eq(type_code))
            .filter(dict_data::Column::Status.eq(dict_data::Model::STATUS_NORMAL))
            .filter(dict_data::Column::DelFlag.eq(dict_data::Model::DEL_FLAG_NORMAL));
        if let Some(after_id) = after_id {
            select = select.filter(dict_data::Column::Id.gt(after_id));
        }
        let data = select
            .order_by_asc(dict_data::Column::Id)
            .limit(limit)
            .all(db)
            .await
            .map_err(database_error)?;
        Ok(Some((dict_type, data)))
    }

    pub async fn dictionary_by_type_code_page<C>(
        &self,
        db: &C,
        tenant_id: &str,
        type_code: &str,
        offset: u64,
        limit: u64,
    ) -> AppResult<Option<AgentDictionaryPage>>
    where
        C: ConnectionTrait,
    {
        let Some(dict_type) = dict_type::Entity::find()
            .filter(dict_type::Column::TenantId.eq(tenant_id))
            .filter(dict_type::Column::Code.eq(type_code))
            .filter(dict_type::Column::Status.eq(dict_type::Model::STATUS_NORMAL))
            .filter(dict_type::Column::DelFlag.eq(dict_type::Model::DEL_FLAG_NORMAL))
            .one(db)
            .await
            .map_err(database_error)?
        else {
            return Ok(None);
        };
        let select = dict_data::Entity::find()
            .filter(dict_data::Column::TenantId.eq(tenant_id))
            .filter(dict_data::Column::TypeCode.eq(type_code))
            .filter(dict_data::Column::Status.eq(dict_data::Model::STATUS_NORMAL))
            .filter(dict_data::Column::DelFlag.eq(dict_data::Model::DEL_FLAG_NORMAL));
        let total = select.clone().count(db).await.map_err(database_error)?;
        let records = select
            .order_by_asc(dict_data::Column::Id)
            .offset(offset)
            .limit(limit)
            .all(db)
            .await
            .map_err(database_error)?;
        Ok(Some(AgentDictionaryPage {
            dict_type,
            records,
            total,
        }))
    }
}

fn empty_page<T>() -> AgentQueryPage<T> {
    AgentQueryPage {
        records: Vec::new(),
        total: 0,
    }
}

fn database_error(error: sea_orm::DbErr) -> AppError {
    AppError::Database(error.to_string())
}

use super::*;

impl AgentService {
    pub(super) async fn query(
        &self,
        transaction: &dyn AgentPersistenceTransaction,
        request: &AgentRequest,
        context: &AuthorizedContext,
    ) -> AppResult<QueryResult> {
        let page = request.page;
        let page_size = request.page_size;
        let offset = page.saturating_sub(1).saturating_mul(page_size);
        let user_dept = context.snapshot.user.as_ref().and_then(|user| user.dept_id);
        match request.capability {
            AgentCapability::Capabilities => {
                let items = AgentCapability::ALL
                    .into_iter()
                    .filter(|capability| *capability != AgentCapability::Capabilities)
                    .filter(|capability| {
                        self.ensure_capability_authorized(capability.descriptor(), context)
                            .is_ok()
                    })
                    .filter(|capability| capability_has_rows(*capability, context, user_dept))
                    .map(|capability| {
                        let descriptor = capability.descriptor();
                        AgentCapabilityVo {
                            key: descriptor.key,
                            method: descriptor.method,
                            path: descriptor.path,
                        }
                    })
                    .collect::<Vec<_>>();
                Ok(QueryResult::new(items.len(), json_value(items)?))
            }
            AgentCapability::DirectoryUsers => {
                let scope = users_scope(
                    &context.account_scope,
                    context.user_scope.as_ref(),
                    user_dept,
                );
                let scope_empty = matches!(&scope, AgentRowScope::Empty);
                let result = transaction
                    .users_page(&context.tenant.tenant_id, scope, offset, page_size)
                    .await?;
                let department_names = context
                    .snapshot
                    .departments
                    .iter()
                    .map(|department| (department.id, department.name.clone()))
                    .collect::<BTreeMap<_, _>>();
                let items = result
                    .records
                    .into_iter()
                    .map(|user| AgentUserVo {
                        id: user.id.to_string(),
                        username: user.username,
                        nickname: user.nickname,
                        dept_name: user
                            .dept_id
                            .and_then(|id| department_names.get(&id).cloned()),
                        status: user.status,
                    })
                    .collect::<Vec<_>>();
                let data = AgentPage::new(
                    items,
                    page,
                    page_size,
                    result.total,
                    self.config.max_page_size,
                );
                Ok(QueryResult::page(data.items.len(), data, scope_empty)?)
            }
            AgentCapability::DirectoryDepartments => {
                let scope = departments_scope(
                    &context.account_scope,
                    context.user_scope.as_ref(),
                    user_dept,
                );
                let scope_empty = matches!(&scope, AgentRowScope::Empty);
                let result = transaction
                    .departments_page(&context.tenant.tenant_id, scope, offset, page_size)
                    .await?;
                let items = result
                    .records
                    .into_iter()
                    .map(|department| AgentDepartmentVo {
                        id: department.id.to_string(),
                        name: department.name,
                        parent_id: department.parent_id.map(|id| id.to_string()),
                        status: department.status,
                    })
                    .collect::<Vec<_>>();
                let data = AgentPage::new(
                    items,
                    page,
                    page_size,
                    result.total,
                    self.config.max_page_size,
                );
                Ok(QueryResult::page(data.items.len(), data, scope_empty)?)
            }
            AgentCapability::DirectoryPosts => {
                if !both_all(context) {
                    return QueryResult::empty_page(page, page_size, self.config.max_page_size);
                }
                let result = transaction
                    .posts_page(&context.tenant.tenant_id, offset, page_size)
                    .await?;
                let items = result
                    .records
                    .into_iter()
                    .map(|post| AgentPostVo {
                        id: post.id.to_string(),
                        code: post.code,
                        name: post.name,
                        status: post.status,
                    })
                    .collect::<Vec<_>>();
                let row_count = items.len();
                let data = AgentPage::new(
                    items,
                    page,
                    page_size,
                    result.total,
                    self.config.max_page_size,
                );
                Ok(QueryResult::new(row_count, json_value(data)?))
            }
            AgentCapability::ReferenceDictionary => {
                if !both_all(context) {
                    return QueryResult::empty_dictionary(
                        request.type_code.clone().unwrap_or_default(),
                        page,
                        page_size,
                        self.config.max_page_size,
                    );
                }
                let type_code = validate_type_code(request.type_code.as_deref())?;
                let result = transaction
                    .dictionary_page(&context.tenant.tenant_id, type_code, offset, page_size)
                    .await?;
                let Some(result) = result else {
                    return Err(AppError::NotFound("字典类型不存在".into()));
                };
                let items = result
                    .records
                    .into_iter()
                    .map(|item| AgentDictionaryItemVo {
                        label: item.label,
                        value: item.value,
                        sort: item.sort,
                    })
                    .collect::<Vec<_>>();
                let row_count = items.len();
                let data = AgentDictionaryVo {
                    type_code: result.type_code,
                    items,
                    page,
                    page_size,
                    total: result.total,
                    total_pages: result.total.div_ceil(page_size),
                    max_page_size: self.config.max_page_size,
                };
                Ok(QueryResult::new(row_count, json_value(data)?))
            }
        }
    }
}

pub(super) struct QueryResult {
    pub(super) data: serde_json::Value,
    pub(super) row_count: usize,
    pub(super) reason_code: &'static str,
}

impl QueryResult {
    fn new(row_count: usize, data: serde_json::Value) -> Self {
        Self {
            data,
            row_count,
            reason_code: if row_count == 0 {
                "data_scope_empty"
            } else {
                "ok"
            },
        }
    }

    fn page<T>(row_count: usize, data: T, scope_empty: bool) -> AppResult<Self>
    where
        T: Serialize,
    {
        Ok(Self {
            data: json_value(data)?,
            row_count,
            reason_code: if scope_empty {
                "data_scope_empty"
            } else {
                "ok"
            },
        })
    }

    fn empty_page(page: u64, page_size: u64, max_page_size: u64) -> AppResult<Self> {
        Ok(Self {
            data: json_value(AgentPage::<AgentPostVo>::new(
                Vec::new(),
                page,
                page_size,
                0,
                max_page_size,
            ))?,
            row_count: 0,
            reason_code: "data_scope_empty",
        })
    }

    fn empty_dictionary(
        type_code: String,
        page: u64,
        page_size: u64,
        max_page_size: u64,
    ) -> AppResult<Self> {
        Ok(Self {
            data: json_value(AgentDictionaryVo {
                type_code,
                items: Vec::new(),
                page,
                page_size,
                total: 0,
                total_pages: 0,
                max_page_size,
            })?,
            row_count: 0,
            reason_code: "data_scope_empty",
        })
    }
}

pub(super) fn validate_subjects(
    snapshot: &AgentAuthorizationSnapshot,
    delegated: bool,
) -> AppResult<()> {
    let account_role_ids = snapshot
        .account_role_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if snapshot
        .roles
        .iter()
        .any(|role| account_role_ids.contains(&role.id) && role.is_super)
    {
        return Err(AppError::Authorization("服务账号不能绑定超级角色".into()));
    }
    if delegated {
        let user = snapshot.user.as_ref().ok_or_else(invalid_credential)?;
        if !user.is_enabled() {
            return Err(invalid_credential());
        }
    }
    Ok(())
}

pub(super) fn subject_permissions(
    snapshot: &AgentAuthorizationSnapshot,
    subject_role_ids: &[i64],
) -> Vec<String> {
    let role_ids = subject_role_ids.iter().copied().collect::<BTreeSet<_>>();
    let active_role_ids = snapshot
        .roles
        .iter()
        .filter(|role| role_ids.contains(&role.id) && role.is_active())
        .map(|role| role.id)
        .collect::<BTreeSet<_>>();
    let permission_ids = snapshot
        .role_permissions
        .iter()
        .filter(|relation| active_role_ids.contains(&relation.role_id))
        .map(|relation| relation.permission_id)
        .collect::<BTreeSet<_>>();
    snapshot
        .permissions
        .iter()
        .filter(|permission| permission_ids.contains(&permission.id) && permission.is_active())
        .map(|permission| permission.code.clone())
        .collect()
}

pub(super) fn both_all(context: &AuthorizedContext) -> bool {
    context.account_scope.is_all() && context.user_scope.as_ref().is_none_or(SubjectScope::is_all)
}

pub(super) fn capability_has_rows(
    capability: AgentCapability,
    context: &AuthorizedContext,
    user_dept: Option<i64>,
) -> bool {
    match capability {
        AgentCapability::Capabilities => true,
        AgentCapability::DirectoryUsers => !matches!(
            users_scope(
                &context.account_scope,
                context.user_scope.as_ref(),
                user_dept,
            ),
            AgentRowScope::Empty
        ),
        AgentCapability::DirectoryDepartments => !matches!(
            departments_scope(
                &context.account_scope,
                context.user_scope.as_ref(),
                user_dept,
            ),
            AgentRowScope::Empty
        ),
        AgentCapability::DirectoryPosts | AgentCapability::ReferenceDictionary => both_all(context),
    }
}

pub(super) fn validate_type_code(type_code: Option<&str>) -> AppResult<&str> {
    let value = type_code
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 100)
        .ok_or_else(|| AppError::Validation("字典类型代码无效".into()))?;
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(AppError::Validation("字典类型代码无效".into()));
    }
    Ok(value)
}

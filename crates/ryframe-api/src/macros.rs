/// 生成标准 ListQuery 结构体（包含 page/page_size + 可选过滤字段）。
///
/// # 示例
/// ```ignore
/// list_query!(pub NoticeListQuery {
///     title: String,
///     notice_type: String,
///     status: String,
/// });
/// ```
#[macro_export]
macro_rules! list_query {
    ($vis:vis $name:ident { $($field:ident: $ty:ty),* $(,)? }) => {
        #[derive(Debug, serde::Deserialize)]
        $vis struct $name {
            #[serde(default)]
            pub page: u64,
            #[serde(default = "ryframe_core::repository::default_page_size", alias = "pageSize")]
            pub page_size: u64,
            $(
                pub $field: Option<$ty>,
            )*
        }
    };
}

/// 生成标准 detail 处理函数体（find_by_id → NotFound）。
///
/// 配合 #[utoipa::path] 使用：
/// ```ignore
/// #[utoipa::path(...)]
/// async fn detail(State(state): State<AppState>, Path(id): Path<i64>) -> AppResult<Json<ApiResponse<NoticeVo>>> {
///     detail_body!(state, id, notice_service, NoticeVo, "通知公告")
/// }
/// ```
#[macro_export]
macro_rules! detail_body {
    ($state:ident, $id:ident, $service:ident, $vo:ty, $entity:literal) => {{
        match $state.$service.find_by_id(&$state.db, $id).await? {
            Some(v) => Ok(axum::Json(ryframe_common::ApiResponse::success(v))),
            None => Err(ryframe_common::AppError::NotFound(format!(
                "{}不存在",
                $entity
            ))),
        }
    }};
}

/// 生成标准 remove 处理函数体（delete → 成功消息）。
///
/// 配合 #[utoipa::path] 使用：
/// ```ignore
/// #[utoipa::path(...)]
/// async fn remove(State(state): State<AppState>, Path(id): Path<i64>) -> AppResult<Json<ApiResponse<()>>> {
///     remove_body!(state, id, notice_service)
/// }
/// ```
#[macro_export]
macro_rules! remove_body {
    ($state:ident, $id:ident, $service:ident) => {{
        $state.$service.delete(&$state.db, $id).await?;
        Ok(axum::Json(
            ryframe_common::ApiResponse::success_no_data_with_msg("删除成功"),
        ))
    }};
}

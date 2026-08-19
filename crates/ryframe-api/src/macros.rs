/// 生成标准 ListQuery 结构体（包含 page/page_size + 可选过滤字段）。
///
/// # 示例
/// ```text
/// use ryframe_api::list_query;
///
/// list_query!(pub NoticeListQuery, NoticeFilterQuery {
///     title: String,
///     notice_type: String,
///     status: String,
/// });
/// ```
#[macro_export]
macro_rules! list_query {
    ($vis:vis $name:ident, $filter_name:ident { $($field:ident: $ty:ty),* $(,)? }) => {
        #[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
        #[serde(deny_unknown_fields)]
        #[into_params(parameter_in = Query)]
        $vis struct $name {
            /// 页码，从 1 开始；未提供时由运行时 TOML 策略解析。
            #[param(minimum = 1)]
            pub page: Option<u64>,
            /// 公共 API 仅接受 snake_case 形式的 `page_size`，并受
            /// `pagination.max_page_size` 限制（默认值为 100）。
            #[param(minimum = 1)]
            pub page_size: Option<u64>,
            $(
                pub $field: Option<$ty>,
            )*
        }

        #[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
        #[serde(deny_unknown_fields)]
        #[into_params(parameter_in = Query)]
        $vis struct $filter_name {
            $(
                pub $field: Option<$ty>,
            )*
        }

        impl $name {
            pub fn into_parts(
                self,
                policy: &ryframe_config::PaginationConfig,
            ) -> $crate::http::HttpResult<(ryframe_adapters::ValidatedPageQuery, $filter_name)> {
                Ok((
                    ryframe_adapters::ValidatedPageQuery::from_optional(
                        self.page,
                        self.page_size,
                        policy,
                    )?,
                    $filter_name {
                        $($field: self.$field),*
                    },
                ))
            }
        }
    };
}

/// 生成标准 detail 处理函数体（find_by_id → NotFound）。
///
/// 配合 #[utoipa::path] 使用：
/// ```text
/// use ryframe_api::detail_body;
/// use ryframe_api::http::{ApiResponse, HttpResult};
///
/// struct NoticeService;
///
/// impl NoticeService {
///     async fn find_by_id(
///         &self,
///         _actor: &ryframe_kernel::ActorContext,
///         _id: i64,
///     ) -> ryframe_kernel::AppResult<Option<String>> {
///         Ok(None)
///     }
/// }
///
/// struct Services {
///     notice: NoticeService,
/// }
///
/// struct AppState {
///     services: Services,
/// }
///
/// async fn detail(
///     state: AppState,
///     actor: ryframe_kernel::ActorContext,
///     id: i64,
/// ) -> HttpResult<axum::Json<ApiResponse<String>>> {
///     detail_body!(state, actor, id, notice, String, "通知公告")
/// }
/// ```
#[macro_export]
macro_rules! detail_body {
    ($state:ident, $actor:ident, $id:ident, $service:ident, $vo:ty, $entity:literal) => {{
        match $state.services.$service.find_by_id(&$actor, $id).await? {
            Some(value) => Ok(axum::Json($crate::http::ApiResponse::<$vo>::success(
                value.into(),
            ))),
            None => Err($crate::http::HttpAppError::from(
                ryframe_kernel::AppError::NotFound(format!("{}不存在", $entity)),
            )),
        }
    }};
}

/// 生成标准 `remove` 处理函数体（删除 → 成功消息）。
///
/// 配合 #[utoipa::path] 使用：
/// ```text
/// use ryframe_api::remove_body;
/// use ryframe_api::http::{ApiResponse, HttpResult};
///
/// struct NoticeService;
///
/// impl NoticeService {
///     async fn delete(
///         &self,
///         _actor: &ryframe_kernel::ActorContext,
///         _id: i64,
///     ) -> ryframe_kernel::AppResult<()> {
///         Ok(())
///     }
/// }
///
/// struct Services {
///     notice: NoticeService,
/// }
///
/// struct AppState {
///     services: Services,
/// }
///
/// async fn remove(
///     state: AppState,
///     actor: ryframe_kernel::ActorContext,
///     id: i64,
/// ) -> HttpResult<axum::Json<ApiResponse<()>>> {
///     remove_body!(state, actor, id, notice)
/// }
/// ```
#[macro_export]
macro_rules! remove_body {
    ($state:ident, $actor:ident, $id:ident, $service:ident) => {{
        $state.services.$service.delete(&$actor, $id).await?;
        Ok(axum::Json($crate::http::ApiResponse::success_no_data()))
    }};
}

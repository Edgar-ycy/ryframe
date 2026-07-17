/// 生成标准 ListQuery 结构体（包含 page/page_size + 可选过滤字段）。
///
/// # 示例
/// ```
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
            #[serde(default = "ryframe_core::repository::default_page")]
            pub page: u64,
            #[serde(default = "ryframe_core::repository::default_page_size")]
            pub page_size: u64,
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
            pub fn into_parts(self) -> (ryframe_core::PageQuery, $filter_name) {
                (
                    ryframe_core::PageQuery {
                        page: self.page,
                        page_size: self.page_size,
                    },
                    $filter_name {
                        $($field: self.$field),*
                    },
                )
            }
        }
    };
}

/// 生成标准 detail 处理函数体（find_by_id → NotFound）。
///
/// 配合 #[utoipa::path] 使用：
/// ```
/// use ryframe_api::detail_body;
/// use ryframe_common::{ApiResponse, AppResult};
///
/// struct NoticeService;
///
/// impl NoticeService {
///     async fn find_by_id(
///         &self,
///         _actor: &ryframe_common::ActorContext,
///         _id: i64,
///     ) -> AppResult<Option<String>> {
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
///     actor: ryframe_common::ActorContext,
///     id: i64,
/// ) -> AppResult<axum::Json<ApiResponse<String>>> {
///     detail_body!(state, actor, id, notice, String, "通知公告")
/// }
/// ```
#[macro_export]
macro_rules! detail_body {
    ($state:ident, $actor:ident, $id:ident, $service:ident, $vo:ty, $entity:literal) => {{
        match $state.services.$service.find_by_id(&$actor, $id).await? {
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
/// ```
/// use ryframe_api::remove_body;
/// use ryframe_common::{ApiResponse, AppResult};
///
/// struct NoticeService;
///
/// impl NoticeService {
///     async fn delete(
///         &self,
///         _actor: &ryframe_common::ActorContext,
///         _id: i64,
///     ) -> AppResult<()> {
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
///     actor: ryframe_common::ActorContext,
///     id: i64,
/// ) -> AppResult<axum::Json<ApiResponse<()>>> {
///     remove_body!(state, actor, id, notice)
/// }
/// ```
#[macro_export]
macro_rules! remove_body {
    ($state:ident, $actor:ident, $id:ident, $service:ident) => {{
        $state.services.$service.delete(&$actor, $id).await?;
        Ok(axum::Json(
            ryframe_common::ApiResponse::success_no_data_with_msg("删除成功"),
        ))
    }};
}

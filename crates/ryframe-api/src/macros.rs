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
            ) -> ryframe_http::HttpResult<(ryframe_core::ValidatedPageQuery, $filter_name)> {
                Ok((
                    ryframe_core::ValidatedPageQuery::from_optional(
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
/// ```
/// use ryframe_api::detail_body;
/// use ryframe_http::{ApiResponse, HttpResult};
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
            Some(value) => Ok(axum::Json(ryframe_http::ApiResponse::<$vo>::success(
                value.into(),
            ))),
            None => Err(ryframe_http::HttpAppError::from(
                ryframe_kernel::AppError::NotFound(format!("{}不存在", $entity)),
            )),
        }
    }};
}

/// 生成标准 `remove` 处理函数体（删除 → 成功消息）。
///
/// 配合 #[utoipa::path] 使用：
/// ```
/// use ryframe_api::remove_body;
/// use ryframe_http::{ApiResponse, HttpResult};
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
        Ok(axum::Json(
            ryframe_http::ApiResponse::success_no_data_with_msg("删除成功"),
        ))
    }};
}

#[cfg(test)]
mod pagination_query_tests {
    use axum::{extract::Query, http::StatusCode, response::IntoResponse};
    use ryframe_config::PaginationConfig;

    crate::list_query!(TestListQuery, TestFilterQuery {});

    #[test]
    fn axum_query_uses_runtime_default_and_rejects_invalid_values() {
        let policy = PaginationConfig {
            default_page_size: 25,
            max_page_size: 100,
        };
        let uri = "/?page=2".parse().unwrap();
        let Query(query) = Query::<TestListQuery>::try_from_uri(&uri).unwrap();
        let (page, _) = query.into_parts(&policy).unwrap();
        assert_eq!(page.page(), 2);
        assert_eq!(page.page_size(), 25);

        let uri = "/?page=0&page_size=101".parse().unwrap();
        let Query(query) = Query::<TestListQuery>::try_from_uri(&uri).unwrap();
        assert!(query.into_parts(&policy).is_err());
    }

    #[test]
    fn axum_query_rejects_legacy_camel_case_page_size() {
        let uri = "/?pageSize=20".parse().unwrap();
        assert!(Query::<TestListQuery>::try_from_uri(&uri).is_err());
    }

    #[test]
    fn api_returns_bad_request_for_every_strict_pagination_boundary() {
        let policy = PaginationConfig::default();

        for uri in [
            "/?page=0&page_size=10",
            "/?page=1&page_size=0",
            "/?page=1&page_size=101",
            "/?page=18446744073709551615&page_size=2",
        ] {
            let uri = uri.parse().unwrap();
            let Query(query) = Query::<TestListQuery>::try_from_uri(&uri).unwrap();
            let response = query.into_parts(&policy).unwrap_err().into_response();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }

        for uri in ["/?page=1&page_size=1", "/?page=1&page_size=100"] {
            let uri = uri.parse().unwrap();
            let Query(query) = Query::<TestListQuery>::try_from_uri(&uri).unwrap();
            assert!(query.into_parts(&policy).is_ok());
        }
    }
}

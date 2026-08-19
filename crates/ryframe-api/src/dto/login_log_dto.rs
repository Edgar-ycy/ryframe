use crate::http::HttpResult;
use ryframe_application::system::LoginInfoQuery;
use ryframe_config::PaginationConfig;
use ryframe_kernel::ValidatedPageQuery;

crate::list_query!(pub LoginLogPageQuery, LoginLogFilterQuery {
    user_name: String,
    status: String,
    begin_time: String,
    end_time: String,
});

impl LoginLogPageQuery {
    pub fn into_service_query(self, policy: &PaginationConfig) -> HttpResult<LoginInfoQuery> {
        let (page, filter) = self.into_parts(policy)?;
        Ok(filter.into_service_query(page))
    }
}

impl LoginLogFilterQuery {
    pub fn into_service_query(self, page: ValidatedPageQuery) -> LoginInfoQuery {
        LoginInfoQuery {
            page,
            user_name: self.user_name,
            status: self.status,
            begin_time: self.begin_time,
            end_time: self.end_time,
        }
    }
}

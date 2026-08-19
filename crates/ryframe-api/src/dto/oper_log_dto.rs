use crate::http::HttpResult;
use ryframe_application::system::OperLogQuery;
use ryframe_config::PaginationConfig;
use ryframe_kernel::ValidatedPageQuery;

crate::list_query!(pub OperLogPageQuery, OperLogFilterQuery {
    oper_name: String,
    status: String,
    begin_time: String,
    end_time: String,
});

impl OperLogPageQuery {
    pub fn into_service_query(self, policy: &PaginationConfig) -> HttpResult<OperLogQuery> {
        let (page, filter) = self.into_parts(policy)?;
        Ok(filter.into_service_query(page))
    }
}

impl OperLogFilterQuery {
    pub fn into_service_query(self, page: ValidatedPageQuery) -> OperLogQuery {
        OperLogQuery {
            page,
            oper_name: self.oper_name,
            status: self.status,
            begin_time: self.begin_time,
            end_time: self.end_time,
        }
    }
}

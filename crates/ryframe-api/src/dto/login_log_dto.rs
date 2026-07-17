use ryframe_core::PageQuery;
use ryframe_service::system::LoginInfoQuery;

crate::list_query!(pub LoginLogPageQuery, LoginLogFilterQuery {
    user_name: String,
    status: String,
    begin_time: String,
    end_time: String,
});

impl LoginLogPageQuery {
    pub fn into_service_query(self) -> LoginInfoQuery {
        let (page, filter) = self.into_parts();
        filter.into_service_query(page)
    }
}

impl LoginLogFilterQuery {
    pub fn into_service_query(self, page: PageQuery) -> LoginInfoQuery {
        LoginInfoQuery {
            page,
            user_name: self.user_name,
            status: self.status,
            begin_time: self.begin_time,
            end_time: self.end_time,
        }
    }
}

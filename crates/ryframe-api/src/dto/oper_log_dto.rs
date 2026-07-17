use ryframe_core::PageQuery;
use ryframe_service::system::OperLogQuery;

crate::list_query!(pub OperLogPageQuery, OperLogFilterQuery {
    oper_name: String,
    status: String,
    begin_time: String,
    end_time: String,
});

impl OperLogPageQuery {
    pub fn into_service_query(self) -> OperLogQuery {
        let (page, filter) = self.into_parts();
        filter.into_service_query(page)
    }
}

impl OperLogFilterQuery {
    pub fn into_service_query(self, page: PageQuery) -> OperLogQuery {
        OperLogQuery {
            page,
            oper_name: self.oper_name,
            status: self.status,
            begin_time: self.begin_time,
            end_time: self.end_time,
        }
    }
}

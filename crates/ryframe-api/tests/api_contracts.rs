#[path = "api_contracts/api_contract.rs"]
mod api_contract;
#[path = "api_contracts/catalog_contract.rs"]
mod catalog_contract;
#[path = "api_contracts/decimal_wire_contract.rs"]
mod decimal_wire_contract;
#[path = "api_contracts/id_parser_contract.rs"]
mod id_parser_contract;
#[path = "api_contracts/integer_wire_contract.rs"]
mod integer_wire_contract;
#[path = "api_contracts/middleware_response_contract.rs"]
mod middleware_response_contract;
#[path = "api_contracts/runtime_contract.rs"]
mod runtime_contract;

// 私有 ID helper 的源码测试复用产品中的整数传输校验。
mod http {
    pub use ryframe_api::http::integer;
}

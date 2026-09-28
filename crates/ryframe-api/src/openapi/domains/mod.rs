use utoipa::openapi::OpenApi;

mod auth;
mod configuration;
mod identity;
mod operations;
mod platform;

pub(super) fn merge(document: &mut OpenApi) {
    document.merge(auth::document());
    document.merge(identity::document());
    document.merge(configuration::document());
    document.merge(operations::document());
    document.merge(platform::document());
}

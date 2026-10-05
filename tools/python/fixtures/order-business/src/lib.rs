pub mod resources;
#[cfg(any(feature = "api", feature = "migration"))]
pub mod generated;

#[cfg(any(feature = "api", feature = "migration"))]
pub fn module() -> ryframe_sdk::RyFrameBusinessModule {
    let builder = ryframe_sdk::BusinessModuleBuilder::new("order")
        .resources(&generated::RESOURCES);
    #[cfg(feature = "api")]
    let builder = builder.routes(generated::routes).openapi(generated::openapi);
    #[cfg(feature = "migration")]
    let builder = builder.migrations(generated::migrations::migrations());
    builder.build()
}

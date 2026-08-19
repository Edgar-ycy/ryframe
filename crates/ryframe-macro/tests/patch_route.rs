use axum::{Router, extract::State};
use ryframe_macro::{patch, route};

#[derive(Clone)]
struct TestState;

#[patch("/items/{id}")]
async fn update(State(_state): State<TestState>) {}

#[test]
fn patch_route_compiles() {
    let _: Router<TestState> = Router::new().merge(route!(update));
}

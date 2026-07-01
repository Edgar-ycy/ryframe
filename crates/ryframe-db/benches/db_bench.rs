use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use ryframe_core::repository::PageQuery;
use sea_orm::{ActiveModelTrait, Database, DatabaseConnection, EntityTrait};

async fn setup_db() -> DatabaseConnection {
    Database::connect("sqlite::memory:")
        .await
        .expect("连接 SQLite 失败")
}

fn bench_db_insert(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("db_insert_user", |b| {
        b.iter_batched(
            || {
                rt.block_on(async {
                    let db = setup_db().await;
                    let dept = ryframe_db::entities::dept::Model {
                        id: 1,
                        tenant_id: "system".into(),
                        name: "测试部门".into(),
                        parent_id: Some(0),
                        ancestors: "0".into(),
                        sort: 0,
                        status: "1".into(),
                        remark: None,
                        del_flag: "0".into(),
                        created_at: chrono::Utc::now(),
                        updated_at: chrono::Utc::now(),
                    };
                    let active: ryframe_db::entities::dept::ActiveModel = dept.into();
                    ryframe_db::entities::dept::Entity::insert(active)
                        .exec(&db)
                        .await
                        .unwrap();
                    db
                })
            },
            |db| {
                rt.block_on(async {
                    let user = ryframe_db::entities::user::Model {
                        id: std::hint::black_box(
                            ryframe_common::utils::snowflake::next_snowflake_id(),
                        ),
                        tenant_id: "system".into(),
                        username: format!("bench_user_{}", rand::random::<u32>()),
                        password_hash: "bench_hash".into(),
                        nickname: "Bench User".into(),
                        email: "bench@test.com".into(),
                        phone: "13800000000".into(),
                        avatar: None,
                        status: "1".into(),
                        auth_version: 1,
                        dept_id: Some(1),
                        remark: None,
                        login_ip: None,
                        login_date: None,
                        del_flag: "0".into(),
                        created_at: chrono::Utc::now(),
                        updated_at: chrono::Utc::now(),
                    };
                    let active: ryframe_db::entities::user::ActiveModel = user.into();
                    active.insert(&db).await.unwrap();
                });
            },
            BatchSize::SmallInput,
        );
    });
}

fn bench_db_select_by_id(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("db_select_user_by_id", |b| {
        let (db, user_id) = rt.block_on(async {
            let db = setup_db().await;
            let dept = ryframe_db::entities::dept::Model {
                id: 1,
                tenant_id: "system".into(),
                name: "测试部门".into(),
                parent_id: Some(0),
                ancestors: "0".into(),
                sort: 0,
                status: "1".into(),
                remark: None,
                del_flag: "0".into(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            };
            let d_active: ryframe_db::entities::dept::ActiveModel = dept.into();
            ryframe_db::entities::dept::Entity::insert(d_active)
                .exec(&db)
                .await
                .unwrap();

            let user = ryframe_db::entities::user::Model {
                id: 1,
                tenant_id: "system".into(),
                username: "bench_user".into(),
                password_hash: "hash".into(),
                nickname: "Bench".into(),
                email: "bench@test.com".into(),
                phone: "13800000000".into(),
                avatar: None,
                status: "1".into(),
                auth_version: 1,
                dept_id: Some(1),
                remark: None,
                login_ip: None,
                login_date: None,
                del_flag: "0".into(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            };
            let u_active: ryframe_db::entities::user::ActiveModel = user.into();
            u_active.insert(&db).await.unwrap();
            (db, 1i64)
        });

        b.iter(|| {
            rt.block_on(async {
                ryframe_db::entities::user::Entity::find_by_id(std::hint::black_box(user_id))
                    .one(&db)
                    .await
                    .unwrap();
            });
        });
    });
}

fn bench_db_pagination(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("db_paginate_100_rows", |b| {
        let db = rt.block_on(async {
            let db = setup_db().await;
            for i in 1..=100 {
                let user = ryframe_db::entities::user::Model {
                    id: i,
                    tenant_id: "system".into(),
                    username: format!("user_{}", i),
                    password_hash: "hash".into(),
                    nickname: format!("User {}", i),
                    email: format!("user{}@test.com", i),
                    phone: "13800000000".into(),
                    avatar: None,
                    status: "1".into(),
                    auth_version: 1,
                    dept_id: None,
                    remark: None,
                    login_ip: None,
                    login_date: None,
                    del_flag: "0".into(),
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                };
                let active: ryframe_db::entities::user::ActiveModel = user.into();
                active.insert(&db).await.unwrap();
            }
            db
        });

        b.iter(|| {
            rt.block_on(async {
                let query = PageQuery {
                    page: 1,
                    page_size: 10,
                };
                let _ = ryframe_db::pagination::paginate(
                    &db,
                    ryframe_db::entities::user::Entity::find(),
                    std::hint::black_box(&query),
                )
                .await
                .unwrap();
            });
        });
    });
}

fn bench_db_update(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("db_update_user", |b| {
        b.iter_batched(
            || {
                rt.block_on(async {
                    let db = setup_db().await;
                    let user = ryframe_db::entities::user::Model {
                        id: 1,
                        tenant_id: "system".into(),
                        username: "update_bench".into(),
                        password_hash: "hash".into(),
                        nickname: "Old Name".into(),
                        email: "old@test.com".into(),
                        phone: "13800000000".into(),
                        avatar: None,
                        status: "1".into(),
                        auth_version: 1,
                        dept_id: None,
                        remark: None,
                        login_ip: None,
                        login_date: None,
                        del_flag: "0".into(),
                        created_at: chrono::Utc::now(),
                        updated_at: chrono::Utc::now(),
                    };
                    let active: ryframe_db::entities::user::ActiveModel = user.into();
                    active.insert(&db).await.unwrap();
                    db
                })
            },
            |db| {
                rt.block_on(async {
                    use sea_orm::{ActiveModelTrait, ActiveValue};
                    let active = ryframe_db::entities::user::ActiveModel {
                        id: ActiveValue::Unchanged(1),
                        nickname: ActiveValue::Set("Updated Name".to_string()),
                        updated_at: ActiveValue::Set(chrono::Utc::now()),
                        ..Default::default()
                    };
                    active.update(&db).await.unwrap();
                });
            },
            BatchSize::SmallInput,
        );
    });
}

criterion_group!(
    benches,
    bench_db_insert,
    bench_db_select_by_id,
    bench_db_pagination,
    bench_db_update,
);
criterion_main!(benches);

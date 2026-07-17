use criterion::{Criterion, criterion_group, criterion_main};
use ryframe_config::AuthConfig;

fn bench_password_hash(c: &mut Criterion) {
    c.bench_function("password_hash_argon2", |b| {
        b.iter(|| {
            let _ = ryframe_auth::password::hash(std::hint::black_box("benchmark_password_123"))
                .expect("hash should succeed");
        });
    });
}

fn bench_password_verify(c: &mut Criterion) {
    let hash = ryframe_auth::password::hash("benchmark_password_123").unwrap();
    c.bench_function("password_verify_correct", |b| {
        b.iter(|| {
            let valid = ryframe_auth::password::verify(
                std::hint::black_box("benchmark_password_123"),
                std::hint::black_box(&hash),
            )
            .unwrap();
            assert!(valid, "password should verify");
        });
    });
}

fn bench_password_verify_wrong(c: &mut Criterion) {
    let hash = ryframe_auth::password::hash("benchmark_password_123").unwrap();
    c.bench_function("password_verify_wrong", |b| {
        b.iter(|| {
            let _ = ryframe_auth::password::verify(
                std::hint::black_box("wrong_password"),
                std::hint::black_box(&hash),
            );
        });
    });
}

fn bench_jwt_encode(c: &mut Criterion) {
    let config = AuthConfig {
        jwt_secret: "bench-jwt-secret-key-for-testing-32bytes".into(),
        access_token_expire: "1h".into(),
        refresh_token_expire: "168h".into(),
        max_login_attempts: 5,
        lockout_duration_minutes: 30,
    };
    let identity = ryframe_auth::jwt::TokenIdentity {
        user_id: 1,
        tenant_id: "system",
        tenant_session_version: 1,
        user_auth_version: 1,
        username: "admin",
    };

    c.bench_function("jwt_encode_access", |b| {
        b.iter(|| {
            let _ = ryframe_auth::jwt::encode_access(
                std::hint::black_box(&identity),
                std::hint::black_box(&config),
            )
            .expect("jwt encode should succeed");
        });
    });
}

fn bench_jwt_decode(c: &mut Criterion) {
    let config = AuthConfig {
        jwt_secret: "bench-jwt-secret-key-for-testing-32bytes".into(),
        access_token_expire: "1h".into(),
        refresh_token_expire: "168h".into(),
        max_login_attempts: 5,
        lockout_duration_minutes: 30,
    };
    let identity = ryframe_auth::jwt::TokenIdentity {
        user_id: 1,
        tenant_id: "system",
        tenant_session_version: 1,
        user_auth_version: 1,
        username: "admin",
    };
    let (token, _) = ryframe_auth::jwt::encode_access(&identity, &config).unwrap();

    c.bench_function("jwt_decode_access", |b| {
        b.iter(|| {
            let _ = ryframe_auth::jwt::decode_token(
                std::hint::black_box(&token),
                std::hint::black_box("bench-jwt-secret-key-for-testing-32bytes"),
            )
            .expect("jwt decode should succeed");
        });
    });
}

// ============ 密码复杂度验证 ============

fn bench_password_complexity_valid(c: &mut Criterion) {
    c.bench_function("password_complexity_valid", |b| {
        b.iter(|| {
            let _ =
                ryframe_auth::password::validate_complexity(std::hint::black_box("StrongP@ss1"));
        });
    });
}

fn bench_password_complexity_invalid(c: &mut Criterion) {
    c.bench_function("password_complexity_invalid", |b| {
        b.iter(|| {
            let _ = ryframe_auth::password::validate_complexity(std::hint::black_box("short"));
        });
    });
}

criterion_group!(
    benches,
    bench_password_hash,
    bench_password_verify,
    bench_password_verify_wrong,
    bench_jwt_encode,
    bench_jwt_decode,
    bench_password_complexity_valid,
    bench_password_complexity_invalid,
);
criterion_main!(benches);

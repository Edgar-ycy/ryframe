# MySQL-only SQLx facade

This package is a narrow facade derived from the public re-exports in SQLx
0.9.0 (`MIT OR Apache-2.0`). RyFrame enables only the runtime APIs required by
SeaORM's MySQL backend. PostgreSQL, SQLite, `Any`, and SQLx compile-time macros
are intentionally absent so those optional drivers cannot enter `Cargo.lock`.

When upgrading SQLx or SeaORM, compare this facade with upstream `sqlx/src/lib.rs`
and run the full MySQL migration, repository, generator, and smoke suites.

Validated dependency set (2026-07-22): SeaORM and `sea-orm-migration` 2.0.0,
`sea-query-sqlx` 0.9.1, and SQLx 0.9.0.

# Local patch

This directory contains the published `proc-macro-error2 2.0.1` library source
under its original MIT or Apache-2.0 license.

RyFrame applies one upstream-compatible change in `src/lib.rs`: the
`proc_macro` extern crate is public so its public re-export no longer triggers
Rust future-incompatibility warning E0365. This is the compiler-suggested fix
for rust-lang/rust#127909.

Remove this patch after an upstream release contains the same fix.

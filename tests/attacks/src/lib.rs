//! Penetration-test regressions and fuzz targets for the Backend Platform, modelled on
//! `labs/minibtc/tests/attacks`. Every test issues a real request against a running
//! `platform-server` (bound on `127.0.0.1:0`, real TCP, real wire) and asserts the
//! refusal; `tests/fuzz.rs` feeds every parser of outside input its committed seed
//! corpus plus generated cases. The suite lives in the workspace so `cargo test
//! --workspace` (and therefore `scripts/check.sh` and CI) runs it against the stack.

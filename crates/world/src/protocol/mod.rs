//! Protocol types shared between world (Rust) and worker/frontend (TS via ts-rs).
//! Add new types in submodules with #[derive(ts_rs::TS)] and
//! #[ts(export, export_to = "../../packages/protocol/dist/")].

mod schema_version;
pub use schema_version::SchemaVersion;

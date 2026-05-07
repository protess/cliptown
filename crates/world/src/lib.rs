pub mod auth;
pub mod backend_catalog;
pub mod config;
pub mod det;
pub mod http;
pub mod loop_;
pub mod protocol;
pub mod state;
pub mod storage;

#[cfg(test)]
mod ts_export {
    /// Forces the ts-rs export side-effect on `cargo test`. Ignored in normal runs;
    /// run via `cargo ts-export` from package.json's `build:rust` script.
    /// Calls `export_all()` so this works under the `--ignored ts_rs_export`
    /// filter (which suppresses the auto-generated `export_bindings_*` tests).
    #[test]
    #[ignore]
    fn ts_rs_export() {
        use crate::protocol::*;
        use crate::state::*;
        use ts_rs::TS;
        let _ = SchemaVersion::CURRENT;
        SchemaVersion::export_all().expect("export SchemaVersion");
        WorkerInbound::export_all().expect("export WorkerInbound");
        WorkerOutbound::export_all().expect("export WorkerOutbound");
        ConsoleInbound::export_all().expect("export ConsoleInbound");
        ConsoleOutbound::export_all().expect("export ConsoleOutbound");
        WorldView::export_all().expect("export WorldView");
        AvatarView::export_all().expect("export AvatarView");
    }
}

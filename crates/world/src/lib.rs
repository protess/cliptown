pub mod agent_supervisor;
pub mod api_skills;
pub mod api_startups;
pub mod auth;
pub mod backend_catalog;
pub mod budget;
pub mod cmd_console;
pub mod cmd_worker;
pub mod config;
pub mod det;
pub mod emit;
pub mod health;
pub mod http;
pub mod loop_;
pub mod mcp_dispatch;
pub mod mcp_http;
pub mod metrics;
pub mod move_sys;
pub mod path;
pub mod permissions;
pub mod persist;
pub mod protocol;
pub mod proximity;
pub mod sandbox;
pub mod scheduler;
pub mod seed;
pub mod skills;
pub mod state;
pub mod storage;
pub mod task_sm;
pub mod view;

#[cfg(test)]
mod ts_export {
    /// Forces the ts-rs export side-effect on `cargo test`. Ignored in normal runs;
    /// run via `cargo ts-export` from package.json's `build:rust` script.
    /// Calls `export_all()` so this works under the `--ignored ts_rs_export`
    /// filter (which suppresses the auto-generated `export_bindings_*` tests).
    #[test]
    #[ignore]
    fn ts_rs_export() {
        use crate::backend_catalog::BackendInfo;
        use crate::protocol::*;
        use crate::state::*;
        use crate::task_sm::TaskStatus;
        use ts_rs::TS;
        let _ = SchemaVersion::CURRENT;
        SchemaVersion::export_all().expect("export SchemaVersion");
        WorkerInbound::export_all().expect("export WorkerInbound");
        WorkerOutbound::export_all().expect("export WorkerOutbound");
        ConsoleInbound::export_all().expect("export ConsoleInbound");
        ConsoleOutbound::export_all().expect("export ConsoleOutbound");
        WorldView::export_all().expect("export WorldView");
        AvatarView::export_all().expect("export AvatarView");
        BackendInfo::export_all().expect("export BackendInfo");
        TaskStatus::export_all().expect("export TaskStatus");
    }
}

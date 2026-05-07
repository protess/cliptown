// build.rs — placeholder. ts-rs `#[ts(export, ...)]` derives run on `cargo test`,
// not on `cargo build`, by design. We trigger them here so a clean `cargo build`
// produces the .d.ts files. See M0.3 Step 3 for the explicit invocation.
fn main() {
    println!("cargo:rerun-if-changed=src/protocol");
}

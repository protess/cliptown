fn main() {
    tracing_subscriber::fmt().json().init();
    tracing::info!(component = "world", event = "boot", "cliptown world starting");
}

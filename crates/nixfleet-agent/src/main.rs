fn main() {
    tracing_subscriber::fmt::init();
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "nixfleet-agent v0.2 skeleton — functional body lands in PR 3"
    );
}

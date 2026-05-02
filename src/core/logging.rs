use tracing_subscriber::EnvFilter;

/// Initialise the global tracing subscriber.
///
/// Reads `KRYPTOS_LOG` (e.g. `KRYPTOS_LOG=debug`) and falls back to
/// `info` for the workspace, `warn` for everything else.
pub fn init() {
    let filter = EnvFilter::try_from_env("KRYPTOS_LOG")
        .unwrap_or_else(|_| EnvFilter::new("warn,kryptos=info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}

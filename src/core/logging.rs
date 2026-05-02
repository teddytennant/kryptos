use tracing_subscriber::EnvFilter;

/// Initialise the global tracing subscriber.
///
/// Reads `SIGVIM_LOG` (e.g. `SIGVIM_LOG=debug`) and falls back to
/// `info` for the workspace, `warn` for everything else.
pub fn init() {
    let filter = EnvFilter::try_from_env("SIGVIM_LOG")
        .unwrap_or_else(|_| EnvFilter::new("warn,sigvim=info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}

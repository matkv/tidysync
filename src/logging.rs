use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::time::ChronoLocal;

/// Initialise logging for CLI subcommands.
///
/// Defaults to `info`; `RUST_LOG=debug tidysync watch` turns on per-event detail.
pub fn init() {
    tracing_subscriber::fmt()
        .with_env_filter(filter())
        .with_timer(ChronoLocal::new("%H:%M:%S".to_string()))
        .with_target(false)
        .init();
}

fn filter() -> EnvFilter {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    // The HTTP stack logs every connection pool operation at debug, which buries
    // our own output under `RUST_LOG=debug`. These directives replace any RUST_LOG
    // set for the same targets, so the HTTP stack is pinned at info regardless.
    ["hyper=info", "hyper_util=info", "reqwest=info", "rustls=info"]
        .iter()
        .fold(filter, |filter, directive| {
            filter.add_directive(directive.parse().expect("static directive is valid"))
        })
}

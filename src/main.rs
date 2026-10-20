use std::io::IsTerminal;
use std::process::ExitCode;
use tracing_subscriber::EnvFilter;
use veil::config::Cli;

fn main() -> ExitCode {
    // Diagnostics go to stderr. Stdout is left for what a command prints as its
    // result. Colours only on a terminal: under systemd stderr is the journal,
    // which would store the escape codes. The journal also has its own time.
    let log = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        );
    if veil::service::stderr_is_journal() {
        log.without_time().init();
    } else {
        log.init();
    }

    let cli = Cli::parse_args();
    let notify = cli.notifies_errors();
    if let Err(e) = veil::run(cli) {
        tracing::error!("{e}");
        if notify && e.wants_notification() {
            veil::notify::notify_error("Veil - Fatal Error", &e.to_string());
        }
        return ExitCode::from(e.exit_code());
    }

    ExitCode::SUCCESS
}

#[cfg(not(feature = "miniserver"))]
compile_error!("build with --features miniserver (or use default features)");

mod mdns_browse;
mod player_state;
mod qobuz;

mod radio;

mod app_config;
mod cast;
mod command_audit;
mod config;
mod connect_bootstrap;
mod coordinator;
mod discovery;
mod health;
mod loxone_config;
mod loxone_wire;
mod miniserver;
mod net_util;
mod recents;
mod remote;
mod room_favs;
mod speaker;
mod voice_upload;
mod wire_response;
mod zone_audio_prefs;
mod zone_enrich;
mod zone_equalizer;
mod zone_publish;
mod zone_registry;
mod zone_transport;
mod zone_volume;

pub use app_config::AppConfig;

use anyhow::{Context, Result};
use clap::Parser;
use tracing::info;

#[derive(Parser)]
#[command(
    name = "loxone-audioserver",
    about = "Software Loxone Audioserver for LAN speakers"
)]
struct Args {
    /// Audioserver serial / MAC (12 hex chars, e.g. 504F94FF1BB3); required to run the daemon
    #[arg(long, env = "AUDIOSERVER_SERIAL", value_parser = parse_serial)]
    serial: Option<String>,
    /// Friendly name in Loxone Config discovery (default: audioserver)
    #[arg(long, env = "AUDIOSERVER_NAME")]
    name: Option<String>,
    /// Hostname field in UDP discovery response
    #[arg(long, env = "AUDIOSERVER_HOSTNAME")]
    hostname: Option<String>,
    /// Host/IP for cover art and HTTP on 7091 (default: auto-detect LAN IPv4)
    #[arg(long, env = "AUDIOSERVER_PUBLIC_HOST")]
    public_host: Option<String>,
    /// Timestamps on log lines; turn off for runtimes that add their own (true/false, yes/no, 1/0)
    #[arg(
        long,
        env = "LOG_TIMESTAMPS",
        default_value = "true",
        action = clap::ArgAction::Set,
        value_parser = clap::builder::BoolishValueParser::new()
    )]
    log_timestamps: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Exit 0 when a running daemon accepts connections on 7091 and 7095 (container healthcheck)
    Health,
}

const SERIAL_HELP: &str =
    "set AUDIOSERVER_SERIAL (or --serial) to the audioserver serial: 12 hex digits, e.g. 504F94FF1BB3";

/// Accepts 12 hex digits, optionally separated by `:` or `-`; anything else (an empty
/// `AUDIOSERVER_SERIAL` in a container definition, a typo) stops startup, since Loxone Config
/// cannot pair with it. Returns the bare upper-case form (`504F94FF1BB3`) that the Miniserver
/// uses as its config key, so zone extraction and extension MACs match it.
fn parse_serial(value: &str) -> Result<String, String> {
    let value = value.trim();
    let only_hex_and_separators = value
        .chars()
        .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '-');
    let digits = value.chars().filter(|c| c.is_ascii_hexdigit()).count();
    if only_hex_and_separators && digits == 12 {
        Ok(discovery::normalize_serial(value))
    } else {
        Err(SERIAL_HELP.to_string())
    }
}

/// An empty environment variable means "not set" (a container definition with a blank value).
fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.trim().is_empty())
}

fn tracing_env_filter() -> tracing_subscriber::EnvFilter {
    use tracing_subscriber::EnvFilter;

    const DEFAULT: &str = concat!(
        "info,",
        "loxone_audioserver::coordinator=info,",
        "loxone_audioserver::miniserver=info,",
        "loxone_audioserver::remote=info,",
        "mdns_sd=off",
    );
    const MDNS_SUPPRESS: &str = "mdns_sd=off";

    match std::env::var("RUST_LOG") {
        Ok(env) if env.contains("mdns_sd=") => {
            EnvFilter::try_new(&env).unwrap_or_else(|_| EnvFilter::new(DEFAULT))
        }
        Ok(env) => {
            let merged = format!("{env},{MDNS_SUPPRESS}");
            EnvFilter::try_new(&merged).unwrap_or_else(|_| EnvFilter::new(DEFAULT))
        }
        Err(_) => EnvFilter::new(DEFAULT),
    }
}

fn init_rustls_crypto() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

#[tokio::main]
async fn main() -> Result<()> {
    // Registered before anything else, so a stop during startup (first-start key generation,
    // config load) is not dropped: as PID 1 in a container there is no default handler.
    let shutdown = shutdown_signal()?;
    init_rustls_crypto();

    let args = Args::parse();
    if let Some(Command::Health) = args.command {
        return health::check().await;
    }
    let Some(serial) = args.serial else {
        use clap::CommandFactory;
        Args::command()
            .error(clap::error::ErrorKind::MissingRequiredArgument, SERIAL_HELP)
            .exit();
    };

    // Container runtimes such as RouterOS add their own timestamp to each log line.
    let timestamps = args.log_timestamps;
    // No colour codes when the output is not a terminal (container logs, files).
    let fmt = tracing_subscriber::fmt()
        .with_env_filter(tracing_env_filter())
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()));
    if timestamps {
        fmt.init();
    } else {
        fmt.without_time().init();
    }

    run_serve_daemon(
        serial,
        non_empty(args.name),
        non_empty(args.hostname),
        non_empty(args.public_host),
        shutdown,
    )
    .await
}

async fn run_serve_daemon(
    serial: String,
    name: Option<String>,
    hostname: Option<String>,
    public_host: Option<String>,
    shutdown: impl std::future::Future<Output = Result<()>>,
) -> Result<()> {
    use tokio::sync::{broadcast, mpsc};

    let app_config = app_config::AppConfig::new(serial, name, hostname, public_host)?;
    info!(
        public_host = %app_config.public_host,
        cover_proxy_base = %app_config.cover_proxy_base(),
        "audioserver public host for cover URLs"
    );
    miniserver::init_heartbeat();
    config::AudioserverConfigFile::init().context("load audioserver-config.json")?;
    zone_volume::init();
    let hw_channels = coordinator::new_hw_channel_ids();
    let (from_all, _) = broadcast::channel(256);
    let (from_api, to_coordinator) = mpsc::channel(32);
    let (speaker_tx, speaker_rx) = mpsc::channel(64);

    discovery::start_discovery(app_config.clone());
    let speaker_discovery =
        speaker::manager::spawn_discovery(speaker_tx.clone(), std::time::Duration::from_secs(60));

    let remote_cfg = app_config.clone();
    let remote_api = from_api.clone();
    let remote_events = from_all.clone();
    let remote = tokio::spawn(async move {
        remote::start(remote_cfg, remote_api, remote_events).await;
    });

    let cfg = app_config.clone();
    let hw = hw_channels.clone();
    let events = from_all.clone();
    let coordinator = tokio::spawn(async move {
        coordinator::run(cfg, to_coordinator, (speaker_tx, speaker_rx), events, hw).await
    });

    let ms_cfg = app_config.clone();
    let ms_api = from_api.clone();
    let ms_events = from_all.clone();
    let ms_hw = hw_channels.clone();
    let miniserver = tokio::spawn(async move {
        miniserver::start(ms_cfg, ms_api, ms_events, ms_hw).await;
    });

    info!(
        serial = %app_config.serial,
        remote_port = 7091_u16,
        miniserver_port = 7095_u16,
        discovery_udp_port = discovery::DISCOVERY_LISTEN_PORT,
        "loxone audioserver daemon running"
    );

    shutdown.await?;
    info!("loxone audioserver daemon stopping");
    coordinator.abort();
    miniserver.abort();
    remote.abort();
    speaker_discovery.abort();
    Ok(())
}

/// SIGINT (Ctrl-C) or SIGTERM (container stop). The handlers are installed when this is
/// called, not when the returned future is first polled, so signals that arrive while the
/// daemon starts are kept until it waits for them.
fn shutdown_signal() -> Result<impl std::future::Future<Output = Result<()>>> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut int = signal(SignalKind::interrupt())?;
        let mut term = signal(SignalKind::terminate())?;
        Ok(async move {
            tokio::select! {
                _ = int.recv() => {}
                _ = term.recv() => {}
            }
            Ok(())
        })
    }
    #[cfg(not(unix))]
    {
        Ok(async {
            tokio::signal::ctrl_c().await?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn args_definition_is_valid() {
        Args::command().debug_assert();
    }

    #[test]
    fn health_subcommand_parses() {
        let args = Args::try_parse_from(["loxone-audioserver", "health"]).unwrap();
        assert!(matches!(args.command, Some(Command::Health)));
    }

    #[test]
    fn serial_must_be_twelve_hex_digits() {
        assert_eq!(parse_serial("504F94FF1BB3").unwrap(), "504F94FF1BB3");
        assert_eq!(parse_serial("50-4f-94-ff-1b-b3").unwrap(), "504F94FF1BB3");
        assert_eq!(parse_serial(" 50:4f:94:ff:1b:b3 ").unwrap(), "504F94FF1BB3");
        for bad in [
            "",
            "  ",
            "xyz",
            "504F94FF1BB",
            "504F94FF1BB3A",
            "504F94FF1BBZ",
        ] {
            assert!(parse_serial(bad).is_err(), "{bad:?} accepted");
        }
        assert!(Args::try_parse_from(["loxone-audioserver", "--serial", ""]).is_err());
    }

    #[test]
    fn log_timestamps_accepts_boolish_values() {
        for (v, want) in [
            ("false", false),
            ("FALSE", false),
            ("0", false),
            ("no", false),
            ("off", false),
            ("true", true),
            ("yes", true),
            ("1", true),
        ] {
            let args = Args::try_parse_from(["loxone-audioserver", "--log-timestamps", v]).unwrap();
            assert_eq!(args.log_timestamps, want, "{v}");
        }
        assert!(Args::try_parse_from(["loxone-audioserver", "--log-timestamps", "maybe"]).is_err());
    }

    #[test]
    fn blank_optional_settings_count_as_unset() {
        assert_eq!(non_empty(Some(" ".into())), None);
        assert_eq!(
            non_empty(Some("10.0.0.5".into())).as_deref(),
            Some("10.0.0.5")
        );
        assert_eq!(non_empty(None), None);
    }

    #[test]
    fn flags_still_parse_without_subcommand() {
        let args = Args::try_parse_from([
            "loxone-audioserver",
            "--serial",
            "504F94000001",
            "--public-host",
            "10.0.0.5",
        ])
        .unwrap();
        assert_eq!(args.serial.as_deref(), Some("504F94000001"));
        assert_eq!(args.public_host.as_deref(), Some("10.0.0.5"));
        assert!(args.command.is_none());
    }
}

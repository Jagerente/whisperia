use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
};

use anyhow::Context;
use clap::Parser;
use whisperia_proto::{
    control::{Name, Password},
    DEFAULT_PORT,
};
use whisperia_server::{cert::format_fingerprint, Server, ServerConfig};

#[derive(Parser)]
#[command(version)]
struct Args {
    #[arg(long, env = "WHISPERIA_NAME", value_parser = Name::new)]
    name: Option<Name>,
    #[arg(long, env = "WHISPERIA_PORT", default_value_t = DEFAULT_PORT)]
    port: u16,
    #[arg(long, env = "WHISPERIA_BIND", default_value_t = IpAddr::V4(Ipv4Addr::UNSPECIFIED))]
    bind: IpAddr,
    #[arg(long, env = "WHISPERIA_PASSWORD", hide_env_values = true, value_parser = Password::new)]
    password: Option<Password>,
    #[arg(long, env = "WHISPERIA_MAX_PEERS", default_value_t = 16)]
    max_peers: usize,
    #[arg(long, env = "WHISPERIA_NO_ADVERTISE")]
    no_advertise: bool,
    #[arg(
        long,
        env = "WHISPERIA_ADVERTISE_ADDR",
        value_name = "IP",
        value_delimiter = ','
    )]
    advertise_addr: Vec<Ipv4Addr>,
    #[arg(long, env = "WHISPERIA_DATA_DIR")]
    data_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "whisperia_server=info,warn".into()),
        )
        .with_target(false)
        .init();

    let name = args
        .name
        .or_else(|| {
            [
                std::env::var("COMPUTERNAME").ok(),
                std::env::var("HOSTNAME").ok(),
                Some("Whisperia".to_owned()),
            ]
            .into_iter()
            .flatten()
            .find_map(|raw| Name::new(&raw).ok())
        })
        .context("no name for the server, pass --name")?;

    let data_dir = match args.data_dir {
        Some(dir) => dir,
        None => directories::ProjectDirs::from("", "", "whisperia")
            .context("cannot determine a data directory, pass --data-dir")?
            .data_local_dir()
            .join("server"),
    };

    let server = Server::start(ServerConfig {
        name: name.clone(),
        bind: SocketAddr::new(args.bind, args.port),
        password: args.password,
        data_dir,
        max_peers: args.max_peers,
        advertise: !args.no_advertise,
        advertise_addrs: args.advertise_addr,
    })?;

    println!(
        "Whisperia server \"{name}\" is listening on UDP port {}",
        args.port
    );
    println!(
        "certificate fingerprint: {}",
        format_fingerprint(&server.fingerprint())
    );
    if args.no_advertise {
        println!("not announced on the network: clients connect by address. Press Ctrl+C to stop.");
    } else {
        println!("clients on this network will find it automatically. Press Ctrl+C to stop.");
    }

    wait_for_stop().await?;
    println!("stopping...");
    server.shutdown().await;
    Ok(())
}

async fn wait_for_stop() -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut terminate = signal(SignalKind::terminate())?;
        tokio::select! {
            r = tokio::signal::ctrl_c() => r?,
            _ = terminate.recv() => {}
        }
    }
    #[cfg(windows)]
    {
        use tokio::signal::windows::{ctrl_break, ctrl_close, ctrl_logoff, ctrl_shutdown};
        let (mut close, mut logoff, mut shutdown, mut brk) = (
            ctrl_close()?,
            ctrl_logoff()?,
            ctrl_shutdown()?,
            ctrl_break()?,
        );
        tokio::select! {
            r = tokio::signal::ctrl_c() => r?,
            _ = brk.recv() => {}
            _ = close.recv() => {}
            _ = logoff.recv() => {}
            _ = shutdown.recv() => {}
        }
    }
    Ok(())
}

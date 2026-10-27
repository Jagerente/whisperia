#![windows_subsystem = "windows"]

use clap::Parser;
use whisperia::{
    gui,
    launch::{Launch, Start},
};

fn main() -> anyhow::Result<()> {
    let launch = Launch::parse();
    let outcome = launch.start().and_then(|start| match start {
        Start::Here(options, launched) => gui::run(*options, &launched),
        Start::Elsewhere => Ok(()),
    });
    if let Err(e) = &outcome {
        tracing::error!("the window could not be shown: {e:#}");
        #[cfg(windows)]
        whisperia::desktop::fatal::show(&format!("Whisperia cannot start: {e:#}"));
    }
    outcome
}

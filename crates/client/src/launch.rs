use std::{path::PathBuf, sync::Arc, sync::Mutex};

use anyhow::Context;

use crate::{
    app::Options,
    audio::{cpal_backend::CpalBackend, fake::FakeBackend, AudioBackend},
    config::{Config, Paths},
    instance::{self, Claim},
    wake::Waker,
};

pub enum Start {
    Here(Box<Options>),
    Elsewhere,
}

#[derive(clap::Parser)]
#[command(version)]
pub struct Launch {
    #[arg(long, value_name = "DIR")]
    pub settings_dir: Option<PathBuf>,
    #[arg(long)]
    pub fake_audio: bool,
}

impl Launch {
    pub fn start(self) -> anyhow::Result<Start> {
        let paths = Paths::resolve(self.settings_dir.clone())
            .context("cannot determine a settings directory, pass --settings-dir")?;
        let waker = Waker::default();
        let instance = match instance::claim(&paths.config, &waker) {
            Ok(Claim::First(instance)) => Ok(instance),
            Ok(Claim::Elsewhere) => return Ok(Start::Elsewhere),
            Err(e) => Err(e),
        };
        init_logging(&paths);
        let instance = instance
            .inspect_err(|e| tracing::warn!("{e}, starting anyway"))
            .ok();
        Ok(Start::Here(Box::new(self.options(paths, instance, waker))))
    }

    fn options(self, paths: Paths, instance: Option<instance::Instance>, waker: Waker) -> Options {
        let (config, warning) = Config::load(&paths.config);
        if let Some(warning) = warning {
            tracing::warn!("{warning}");
        }
        let backend: Arc<dyn AudioBackend> = if self.fake_audio {
            let mut fake = FakeBackend::new(48_000, Some((440.0, 0.2)));
            fake.record = false;
            Arc::new(fake)
        } else {
            Arc::new(CpalBackend)
        };
        let desktop = crate::desktop::start(&config.hotkeys, &waker);
        Options {
            paths,
            config,
            backend,
            screen: crate::screen::default_backend(),
            previews: Arc::new(crate::preview::Web::default()),
            connect: None,
            desktop,
            instance,
            waker,
            search: crate::discovery::Search::Lan,
        }
    }
}

fn init_logging(paths: &Paths) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "whisperia=info,warn".into());
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false);
    let file = paths
        .log
        .parent()
        .and_then(|dir| std::fs::create_dir_all(dir).ok())
        .and_then(|()| {
            let _ = std::fs::rename(&paths.log, paths.log.with_extension("previous.log"));
            std::fs::File::create(&paths.log).ok()
        });
    match file {
        Some(file) => builder
            .with_ansi(false)
            .with_writer(Mutex::new(file))
            .init(),
        None => builder.with_writer(std::io::sink).init(),
    }
}

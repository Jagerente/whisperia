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
    Here(Box<Options>, Launched),
    Elsewhere,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Launched {
    settings_dir: Option<PathBuf>,
    fake_audio: bool,
    pub hidden: bool,
}

impl Launched {
    pub fn command_line(&self, hidden: bool) -> String {
        let mut arguments = Vec::new();
        if let Some(dir) = &self.settings_dir {
            arguments.push("--settings-dir".to_owned());
            arguments.push(dir.to_string_lossy().into_owned());
        }
        if self.fake_audio {
            arguments.push("--fake-audio".to_owned());
        }
        if hidden {
            arguments.push("--hidden".to_owned());
        }
        arguments
            .iter()
            .map(String::as_str)
            .map(quoted)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn quoted(argument: &str) -> String {
    if !argument.is_empty() && !argument.contains([' ', '\t', '"']) {
        return argument.to_owned();
    }
    let mut quoted = String::from('"');
    let mut backslashes = 0;
    for c in argument.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                quoted.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            }
            _ => {
                quoted.extend(std::iter::repeat_n('\\', backslashes));
                quoted.push(c);
                backslashes = 0;
            }
        }
    }
    quoted.extend(std::iter::repeat_n('\\', backslashes * 2));
    quoted.push('"');
    quoted
}

#[derive(clap::Parser)]
#[command(version)]
pub struct Launch {
    #[arg(long, value_name = "DIR")]
    pub settings_dir: Option<PathBuf>,
    #[arg(long)]
    pub fake_audio: bool,
    #[arg(long)]
    pub hidden: bool,
}

impl Launch {
    pub fn start(mut self) -> anyhow::Result<Start> {
        self.settings_dir = self
            .settings_dir
            .map(|dir| std::path::absolute(&dir).unwrap_or(dir));
        let launched = Launched {
            settings_dir: self.settings_dir.clone(),
            fake_audio: self.fake_audio,
            hidden: self.hidden,
        };
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
        Ok(Start::Here(
            Box::new(self.options(paths, instance, waker)),
            launched,
        ))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn launched(settings_dir: Option<&str>, fake_audio: bool) -> Launched {
        Launched {
            settings_dir: settings_dir.map(PathBuf::from),
            fake_audio,
            hidden: false,
        }
    }

    #[test]
    fn a_plain_copy_comes_back_with_nothing_or_only_hidden() {
        assert_eq!(launched(None, false).command_line(false), "");
        assert_eq!(launched(None, false).command_line(true), "--hidden");
    }

    #[test]
    fn a_copy_comes_back_with_its_own_settings_and_test_tone() {
        assert_eq!(
            launched(Some(r"D:\My Settings"), true).command_line(true),
            r#"--settings-dir "D:\My Settings" --fake-audio --hidden"#
        );
    }

    #[test]
    fn a_folder_ending_in_a_backslash_keeps_its_closing_quote() {
        assert_eq!(
            launched(Some(r"D:\My Settings\"), false).command_line(false),
            r#"--settings-dir "D:\My Settings\\""#
        );
    }

    #[test]
    fn a_quote_inside_a_folder_name_is_escaped_with_the_backslashes_before_it() {
        assert_eq!(quoted(r#"a "b\"c"#), r#""a \"b\\\"c""#);
    }

    #[test]
    fn backslashes_inside_a_folder_name_stay_as_they_are() {
        assert_eq!(quoted(r"C:\a b\c"), r#""C:\a b\c""#);
    }
}

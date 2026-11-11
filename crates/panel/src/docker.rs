//! Control of the bot containers through the Docker Engine API.
//!
//! Every operation first inspects the container and checks its
//! `win.enderr.dc.bot` label: the panel only ever touches containers that
//! were explicitly declared as belonging to that bot.
//!
//! A bot's container is looked up by the name in `bot.toml`, then by its
//! label: platforms such as Coolify give containers their own names.

use std::{collections::HashMap, time::Duration};

use bollard::{
    Docker,
    errors::Error,
    models::{ContainerStateStatusEnum as State, HealthStatusEnum as Health},
    query_parameters::{
        ListContainersOptions, LogsOptions, RestartContainerOptions, StopContainerOptions,
    },
};
use dc_core::registry::{BotDef, CONTAINER_LABEL};
use futures_util::StreamExt;

const INSPECT_TIMEOUT: Duration = Duration::from_secs(5);
const ACTION_TIMEOUT: Duration = Duration::from_secs(40);
/// Seconds Docker waits for a graceful shutdown before killing the bot.
const STOP_GRACE_SECS: i32 = 10;
const LOG_LINES: &str = "300";
const LOG_MAX_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Running {
        /// RFC 3339 start time, as reported by Docker.
        since: Option<String>,
        health: Option<HealthState>,
    },
    Restarting,
    Paused,
    Stopped {
        exit_code: Option<i64>,
        error: Option<String>,
    },
    /// The container does not exist (not deployed yet).
    Missing,
    /// A container with that name exists but is not labelled for this bot.
    Unmanaged,
    /// Docker cannot be reached.
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthState {
    Starting,
    Healthy,
    Unhealthy,
}

impl Status {
    pub fn can_start(&self) -> bool {
        matches!(self, Self::Stopped { .. })
    }

    pub fn can_stop(&self) -> bool {
        matches!(self, Self::Running { .. } | Self::Restarting | Self::Paused)
    }

    pub fn is_controllable(&self) -> bool {
        !matches!(self, Self::Missing | Self::Unmanaged | Self::Unavailable)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Action {
    Start,
    Stop,
    Restart,
}

#[derive(Debug, thiserror::Error)]
pub enum ActionError {
    #[error("Docker is not reachable from the panel.")]
    Unavailable,
    #[error("This bot has not been deployed yet.")]
    Missing,
    #[error("This container is not registered for this bot.")]
    Unmanaged,
    #[error("Docker took too long to respond.")]
    Timeout,
    #[error("Docker refused the action: {0}")]
    Refused(String),
}

pub struct LogLine {
    pub time: Option<String>,
    pub text: String,
}

#[derive(Clone)]
pub struct DockerClient(Option<Docker>);

impl DockerClient {
    pub fn connect() -> Self {
        match Docker::connect_with_local_defaults() {
            Ok(docker) => Self(Some(docker)),
            Err(error) => {
                tracing::warn!(%error, "Docker client unavailable, bot control disabled");
                Self(None)
            }
        }
    }

    /// Id of the bot's container: the one named in `bot.toml`, else the most
    /// recent container labelled with the bot's slug.
    async fn find(&self, docker: &Docker, bot: &BotDef) -> Result<Option<String>, Error> {
        match docker.inspect_container(&bot.container, None).await {
            Ok(info) => return Ok(info.id.or_else(|| Some(bot.container.clone()))),
            Err(Error::DockerResponseServerError {
                status_code: 404, ..
            }) => {}
            Err(error) => return Err(error),
        }
        let filters = HashMap::from([(
            "label".to_owned(),
            vec![format!("{CONTAINER_LABEL}={}", bot.slug)],
        )]);
        let options = ListContainersOptions {
            all: true,
            filters: Some(filters),
            ..Default::default()
        };
        let mut found = docker.list_containers(Some(options)).await?;
        found.sort_by_key(|container| std::cmp::Reverse(container.created.unwrap_or(0)));
        Ok(found.into_iter().find_map(|container| container.id))
    }

    pub async fn status(&self, bot: &BotDef) -> Status {
        self.locate(bot).await.1
    }

    /// The container id and its status.
    async fn locate(&self, bot: &BotDef) -> (Option<String>, Status) {
        let Some(docker) = &self.0 else {
            return (None, Status::Unavailable);
        };
        let id = match tokio::time::timeout(INSPECT_TIMEOUT, self.find(docker, bot)).await {
            Err(_) => return (None, Status::Unavailable),
            Ok(Err(error)) => {
                tracing::warn!(bot = %bot.slug, %error, "container lookup failed");
                return (None, Status::Unavailable);
            }
            Ok(Ok(None)) => return (None, Status::Missing),
            Ok(Ok(Some(id))) => id,
        };
        let status = self.inspect(docker, bot, &id).await;
        (Some(id), status)
    }

    async fn inspect(&self, docker: &Docker, bot: &BotDef, id: &str) -> Status {
        let inspected =
            tokio::time::timeout(INSPECT_TIMEOUT, docker.inspect_container(id, None)).await;
        let info = match inspected {
            Err(_) => return Status::Unavailable,
            Ok(Err(Error::DockerResponseServerError {
                status_code: 404, ..
            })) => return Status::Missing,
            Ok(Err(error)) => {
                tracing::warn!(bot = %bot.slug, %error, "container inspection failed");
                return Status::Unavailable;
            }
            Ok(Ok(info)) => info,
        };

        let label = info
            .config
            .and_then(|config| config.labels)
            .and_then(|labels| labels.get(CONTAINER_LABEL).cloned());
        if label.as_deref() != Some(bot.slug.as_str()) {
            return Status::Unmanaged;
        }

        let state = info.state.unwrap_or_default();
        match state.status {
            Some(State::RUNNING) => Status::Running {
                since: state.started_at,
                health: state.health.and_then(|h| h.status).and_then(|h| match h {
                    Health::STARTING => Some(HealthState::Starting),
                    Health::HEALTHY => Some(HealthState::Healthy),
                    Health::UNHEALTHY => Some(HealthState::Unhealthy),
                    _ => None,
                }),
            },
            Some(State::RESTARTING) => Status::Restarting,
            Some(State::PAUSED) => Status::Paused,
            _ => Status::Stopped {
                exit_code: state.exit_code.filter(|_| state.finished_at.is_some()),
                error: state.error.filter(|e| !e.is_empty()),
            },
        }
    }

    pub async fn run(&self, bot: &BotDef, action: Action) -> Result<(), ActionError> {
        let docker = self.0.as_ref().ok_or(ActionError::Unavailable)?;
        let id = match self.locate(bot).await {
            (_, Status::Unavailable) => return Err(ActionError::Unavailable),
            (_, Status::Missing) | (None, _) => return Err(ActionError::Missing),
            (_, Status::Unmanaged) => return Err(ActionError::Unmanaged),
            (Some(id), _) => id,
        };

        let name = id.as_str();
        let request = async {
            match action {
                Action::Start => docker.start_container(name, None).await,
                Action::Stop => {
                    let options = StopContainerOptions {
                        t: Some(STOP_GRACE_SECS),
                        ..Default::default()
                    };
                    docker.stop_container(name, Some(options)).await
                }
                Action::Restart => {
                    let options = RestartContainerOptions {
                        t: Some(STOP_GRACE_SECS),
                        ..Default::default()
                    };
                    docker.restart_container(name, Some(options)).await
                }
            }
        };
        match tokio::time::timeout(ACTION_TIMEOUT, request).await {
            Err(_) => Err(ActionError::Timeout),
            // 304: already started / already stopped.
            Ok(Ok(()))
            | Ok(Err(Error::DockerResponseServerError {
                status_code: 304, ..
            })) => Ok(()),
            Ok(Err(Error::DockerResponseServerError { message, .. })) => {
                Err(ActionError::Refused(message))
            }
            Ok(Err(error)) => {
                tracing::warn!(bot = %bot.slug, %error, "container action failed");
                Err(ActionError::Unavailable)
            }
        }
    }

    /// Last lines of the container output (stdout and stderr).
    pub async fn logs(&self, bot: &BotDef) -> Result<Vec<LogLine>, ActionError> {
        let docker = self.0.as_ref().ok_or(ActionError::Unavailable)?;
        let id = match self.locate(bot).await {
            (_, Status::Unavailable) => return Err(ActionError::Unavailable),
            (_, Status::Missing) | (None, _) => return Err(ActionError::Missing),
            (_, Status::Unmanaged) => return Err(ActionError::Unmanaged),
            (Some(id), _) => id,
        };

        let options = LogsOptions {
            stdout: true,
            stderr: true,
            timestamps: true,
            tail: LOG_LINES.into(),
            ..Default::default()
        };
        let collect = async {
            let mut raw = String::new();
            let mut stream = docker.logs(&id, Some(options));
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(output) => raw.push_str(&output.to_string()),
                    Err(error) => {
                        tracing::warn!(bot = %bot.slug, %error, "reading logs failed");
                        break;
                    }
                }
                if raw.len() > LOG_MAX_BYTES {
                    break;
                }
            }
            raw
        };
        let raw = tokio::time::timeout(ACTION_TIMEOUT, collect)
            .await
            .map_err(|_| ActionError::Timeout)?;
        Ok(parse_logs(&raw))
    }
}

fn parse_logs(raw: &str) -> Vec<LogLine> {
    raw.lines()
        .map(strip_ansi)
        .map(|line| {
            // Docker prefixes each line with an RFC 3339 timestamp.
            match line.split_once(' ') {
                Some((ts, text)) if ts.len() >= 20 && ts.as_bytes().get(10) == Some(&b'T') => {
                    LogLine {
                        time: Some(ts[..19].replace('T', " ")),
                        text: text.to_owned(),
                    }
                }
                _ => LogLine {
                    time: None,
                    text: line,
                },
            }
        })
        .collect()
}

/// Removes terminal color codes (`ESC [ ... letter`).
fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_are_parsed() {
        let raw = "2026-11-11T10:12:33.123456789Z \u{1b}[32mINFO\u{1b}[0m ready\nplain line\n";
        let lines = parse_logs(raw);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].time.as_deref(), Some("2026-11-11 10:12:33"));
        assert_eq!(lines[0].text, "INFO ready");
        assert_eq!(lines[1].time, None);
        assert_eq!(lines[1].text, "plain line");
    }

    #[test]
    fn permissions_follow_state() {
        let stopped = Status::Stopped {
            exit_code: Some(0),
            error: None,
        };
        assert!(stopped.can_start() && !stopped.can_stop());
        assert!(Status::Restarting.can_stop());
        assert!(!Status::Missing.is_controllable());
        assert!(!Status::Unmanaged.can_start());
    }
}

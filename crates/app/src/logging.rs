//! Process-wide file logging for support diagnostics.

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::fmt;
use tracing_subscriber::EnvFilter;

pub(crate) struct LoggingGuard {
    _worker: WorkerGuard,
}

pub(crate) fn init(data_dir: &Path) -> anyhow::Result<LoggingGuard> {
    let logs = data_dir.join("logs");
    std::fs::create_dir_all(&logs)?;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("steward.log")
        .max_log_files(7)
        .build(&logs)?;
    let (writer, worker) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(writer)
        .try_init()
        .map_err(|error| anyhow::anyhow!("initialize tracing subscriber: {error}"))?;

    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|location| location.to_string())
            .unwrap_or_else(|| "unknown".into());
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
            .unwrap_or("non-string panic payload");
        tracing::error!(
            location = %location,
            payload,
            "application panic"
        );
        previous(info);
    }));

    Ok(LoggingGuard { _worker: worker })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initializes_the_file_subscriber() {
        let dir = std::env::temp_dir().join(format!(
            "steward-logging-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let guard = init(&dir).unwrap();
        tracing::info!("logging test message");
        drop(guard);
        assert!(dir.join("logs").is_dir());
        std::fs::remove_dir_all(dir).ok();
    }
}

//! Structured JSON log file output for Loki/Promtail ingestion.
//!
//! Produces a non-blocking writer backed by `tracing-appender` rolling file
//! appender.  The caller adds the returned writer to a `tracing_subscriber`
//! JSON layer so every log event is emitted as a self-contained JSON line.

use openfang_types::config::{LogRotation, LoggingConfig};
use std::path::PathBuf;

/// Create a non-blocking JSON log writer and its flush guard.
///
/// The guard **must** be kept alive for the lifetime of the application
/// (typically via `std::mem::forget` or storing it in a static).  Dropping
/// it flushes pending writes but also stops the background writer thread.
pub fn make_json_appender(
    logging: &LoggingConfig,
) -> (
    tracing_appender::non_blocking::NonBlocking,
    tracing_appender::non_blocking::WorkerGuard,
) {
    let log_dir = logging
        .json_dir
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join(".openfang")
                .join("logs")
        });
    let _ = std::fs::create_dir_all(&log_dir);

    let file_appender = match logging.rotation {
        LogRotation::Daily => tracing_appender::rolling::daily(&log_dir, &logging.json_file_prefix),
        LogRotation::Hourly => {
            tracing_appender::rolling::hourly(&log_dir, &logging.json_file_prefix)
        }
        LogRotation::Never => tracing_appender::rolling::never(
            &log_dir,
            format!("{}.json.log", logging.json_file_prefix),
        ),
    };
    tracing_appender::non_blocking(file_appender)
}

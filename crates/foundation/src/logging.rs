use crate::CoreError;
use std::sync::Mutex;
use tracing_subscriber::{EnvFilter, fmt::writer::BoxMakeWriter};

/// Debug builds log engine crates at `debug`; everything else at `info`.
const DEFAULT_FILTER: &str = if cfg!(debug_assertions) {
    "info,foundation=debug,platform=debug,rhi=debug,rhi_vulkan=debug"
} else {
    "info"
};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Format {
    Pretty,
    Json,
}

impl Format {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pretty => "pretty",
            Self::Json => "json",
        }
    }
}

fn parse_filter(raw: Option<&str>) -> Result<(EnvFilter, String), CoreError> {
    let s = raw.unwrap_or(DEFAULT_FILTER);
    let f = EnvFilter::try_new(s).map_err(|e| CoreError::InvalidLogFilter(e.to_string()))?;
    Ok((f, s.to_owned()))
}

fn parse_format(raw: Option<&str>) -> Result<Format, CoreError> {
    match raw {
        None | Some("" | "pretty") => Ok(Format::Pretty),
        Some("json") => Ok(Format::Json),
        Some(other) => Err(CoreError::InvalidLogFormat(other.to_owned())),
    }
}

/// Empty value means unset.
fn parse_file(raw: Option<&str>) -> Option<&str> {
    raw.filter(|p| !p.is_empty())
}

/// Sets `s` as the global subscriber, adding the Tracy layer with feature `profile-tracy`.
/// False if one was already installed.
fn install<S>(s: S) -> bool
where
    S: tracing::Subscriber
        + for<'a> tracing_subscriber::registry::LookupSpan<'a>
        + Send
        + Sync
        + 'static,
{
    #[cfg(feature = "profile-tracy")]
    let s = {
        use tracing_subscriber::layer::SubscriberExt;
        s.with(tracing_tracy::TracyLayer::default())
    };
    tracing::subscriber::set_global_default(s).is_ok()
}

/// Installs the global `tracing` subscriber.
///
/// Environment (ADR-0009 §1):
/// - `ANVIL_LOG`: filter directives (default: engine crates at `debug` in debug builds, else `info`).
/// - `ANVIL_LOG_FORMAT`: `pretty` (default) or `json` (one JSON event per line, with span list).
/// - `ANVIL_LOG_FILE`: path to create/truncate and log to, instead of stderr.
///
/// With cargo feature `profile-tracy` spans are also streamed to Tracy (ADR-0013 §2).
///
/// Idempotent: if a global subscriber already exists this is a no-op returning `Ok`.
///
/// # Errors
/// [`CoreError::InvalidLogFilter`], [`CoreError::InvalidLogFormat`] for bad env values;
/// [`CoreError::LogFile`] if the log file cannot be created.
pub fn init_logging() -> Result<(), CoreError> {
    // Checked first so a repeat call does not re-truncate the log file.
    if tracing::dispatcher::has_been_set() {
        return Ok(());
    }
    let var = |k| std::env::var(k).ok();
    let (filter, filter_str) = parse_filter(var("ANVIL_LOG").as_deref())?;
    let format = parse_format(var("ANVIL_LOG_FORMAT").as_deref())?;
    let file_var = var("ANVIL_LOG_FILE");
    let file = parse_file(file_var.as_deref());
    let writer = match file {
        Some(path) => {
            let f = std::fs::File::create(path).map_err(|e| CoreError::LogFile {
                path: path.to_owned(),
                reason: e.to_string(),
            })?;
            BoxMakeWriter::new(Mutex::new(f))
        }
        None => BoxMakeWriter::new(std::io::stderr),
    };
    // The `ansi` feature of tracing-subscriber is off, so output never has colors.
    let b = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_writer(writer);
    // Err means a subscriber is already installed, which is fine.
    let installed = match format {
        Format::Pretty => install(b.finish()),
        Format::Json => install(
            b.json()
                .with_current_span(true)
                .with_span_list(true)
                .finish(),
        ),
    };
    if installed {
        tracing::info!(
            filter = %filter_str,
            format = format.as_str(),
            destination = file.unwrap_or("stderr"),
            "logging initialised"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twice_does_not_panic() {
        init_logging().unwrap();
        init_logging().unwrap();
    }

    #[test]
    fn format_parsing() {
        assert_eq!(parse_format(None).unwrap(), Format::Pretty);
        assert_eq!(parse_format(Some("pretty")).unwrap(), Format::Pretty);
        assert_eq!(parse_format(Some("json")).unwrap(), Format::Json);
        assert!(matches!(
            parse_format(Some("xml")),
            Err(CoreError::InvalidLogFormat(v)) if v == "xml"
        ));
    }

    #[test]
    fn filter_parsing() {
        assert_eq!(parse_filter(None).unwrap().1, DEFAULT_FILTER);
        assert_eq!(
            parse_filter(Some("vulkan::validation=trace")).unwrap().1,
            "vulkan::validation=trace"
        );
        assert!(matches!(
            parse_filter(Some("=[bad")),
            Err(CoreError::InvalidLogFilter(_))
        ));
    }

    #[test]
    fn file_parsing() {
        assert_eq!(parse_file(None), None);
        assert_eq!(parse_file(Some("")), None);
        assert_eq!(parse_file(Some("a.log")), Some("a.log"));
    }
}

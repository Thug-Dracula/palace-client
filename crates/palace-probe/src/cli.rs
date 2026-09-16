//! Command-line parsing. Hand-rolled so the probe has no argument-parsing
//! dependency beyond `palace-wire`.

use std::path::PathBuf;
use std::time::Duration;

/// Parsed command-line options.
#[derive(Debug, Clone)]
pub struct Args {
    /// Server hostname or IP.
    pub host: String,
    /// Server port.
    pub port: u16,
    /// User name to log on with.
    pub user: String,
    /// Where to write the captured fixture, if anywhere.
    pub capture: Option<PathBuf>,
    /// Overall socket timeout used by the wait loops.
    pub timeout: Duration,
    /// Silence duration that ends a read burst.
    pub quiet: Duration,
    /// Hard cap on a read burst.
    pub hard: Duration,
    /// Room to request on logon (0 = server default).
    pub desired_room: i16,
    /// Print every frame, not just the interesting ones.
    pub verbose: bool,
    /// Emit a JSON summary instead of prose.
    pub json: bool,
    /// Print the opcode table and exit.
    pub list_opcodes: bool,
}

impl Default for Args {
    fn default() -> Self {
        Args {
            host: "localhost".to_string(),
            port: 9998,
            user: "probe".to_string(),
            capture: None,
            timeout: Duration::from_secs(5),
            quiet: Duration::from_millis(2500),
            hard: Duration::from_secs(20),
            desired_room: 0,
            verbose: false,
            json: false,
            list_opcodes: false,
        }
    }
}

/// Usage text.
pub fn usage() -> String {
    "\
palace-probe — headless Palace protocol probe

USAGE:
    palace-probe [OPTIONS]

OPTIONS:
    --host <HOST>            Server hostname or IP        [default: localhost]
    --port <PORT>            Server port                  [default: 9998]
    --user <NAME>            User name to log on with     [default: probe]
    --capture <DIR>          Write a replayable fixture corpus to DIR
    --desired-room <ID>      Request this room on logon   [default: 0]
    --timeout <SECONDS>      Socket timeout               [default: 5]
    --quiet <SECONDS>        Silence that ends a burst    [default: 2.5]
    --hard <SECONDS>         Hard cap on a burst          [default: 20]
    --verbose                Print every frame
    --json                   Emit a JSON summary
    --list-opcodes           Print the known opcode table and exit
    -h, --help               Show this help

EXAMPLES:
    palace-probe --host localhost --port 9998 --user probe
    palace-probe --host localhost --port 9998 --user probe --capture ../fixtures/logon-run1
"
    .to_string()
}

/// Why argument parsing did not produce an [`Args`].
#[derive(Debug, Clone)]
pub enum CliError {
    /// `--help` was requested; print the message and exit 0.
    Help(String),
    /// The command line was invalid; print the message and exit 2.
    Invalid(String),
}

impl CliError {
    /// The message to print.
    pub fn message(&self) -> &str {
        match self {
            CliError::Help(m) | CliError::Invalid(m) => m,
        }
    }

    /// The process exit code that fits this outcome.
    pub fn exit_code(&self) -> i32 {
        match self {
            CliError::Help(_) => 0,
            CliError::Invalid(_) => 2,
        }
    }
}

/// Parse arguments.
pub fn parse<I: IntoIterator<Item = String>>(iter: I) -> Result<Args, CliError> {
    let mut args = Args::default();
    let mut it = iter.into_iter();
    while let Some(raw) = it.next() {
        let (flag, inline) = match raw.split_once('=') {
            Some((f, v)) => (f.to_string(), Some(v.to_string())),
            None => (raw.clone(), None),
        };
        let mut value = |name: &str| -> Result<String, CliError> {
            inline
                .clone()
                .or_else(|| it.next())
                .ok_or_else(|| CliError::Invalid(format!("{name} requires a value")))
        };
        match flag.as_str() {
            "-h" | "--help" => return Err(CliError::Help(usage())),
            "--host" => args.host = value("--host")?,
            "--port" => {
                let v = value("--port")?;
                args.port = v
                    .parse()
                    .map_err(|_| CliError::Invalid(format!("invalid --port {v:?}")))?;
            }
            "--user" | "--name" => args.user = value("--user")?,
            "--capture" => args.capture = Some(PathBuf::from(value("--capture")?)),
            "--desired-room" => {
                let v = value("--desired-room")?;
                args.desired_room = v
                    .parse()
                    .map_err(|_| CliError::Invalid(format!("invalid --desired-room {v:?}")))?;
            }
            "--timeout" => args.timeout = secs(&value("--timeout")?, "--timeout")?,
            "--quiet" => args.quiet = secs(&value("--quiet")?, "--quiet")?,
            "--hard" => args.hard = secs(&value("--hard")?, "--hard")?,
            "--verbose" | "-v" => args.verbose = true,
            "--json" => args.json = true,
            "--list-opcodes" => args.list_opcodes = true,
            other => {
                return Err(CliError::Invalid(format!(
                    "unknown option {other:?}\n\n{}",
                    usage()
                )))
            }
        }
    }
    if args.user.is_empty() {
        return Err(CliError::Invalid("--user must not be empty".to_string()));
    }
    Ok(args)
}

fn secs(v: &str, name: &str) -> Result<Duration, CliError> {
    let secs: f64 = v
        .parse()
        .map_err(|_| CliError::Invalid(format!("invalid {name} {v:?}")))?;
    if !(secs.is_finite() && secs > 0.0) {
        return Err(CliError::Invalid(format!(
            "{name} must be a positive number"
        )));
    }
    Ok(Duration::from_secs_f64(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_str(s: &str) -> Result<Args, CliError> {
        parse(s.split_whitespace().map(str::to_string))
    }

    #[test]
    fn applies_documented_defaults() {
        let a = parse_str("").unwrap();
        assert_eq!(a.host, "localhost");
        assert_eq!(a.port, 9998);
        assert_eq!(a.user, "probe");
        assert_eq!(a.desired_room, 0);
    }

    #[test]
    fn parses_both_flag_spellings() {
        let a = parse_str("--host localhost --port 9998 --user Rico").unwrap();
        assert_eq!(a.host, "localhost");
        assert_eq!(a.port, 9998);
        assert_eq!(a.user, "Rico");

        let b = parse_str("--host=localhost --port=9998 --user=Rico").unwrap();
        assert_eq!(b.port, a.port);
        assert_eq!(b.user, a.user);
    }

    #[test]
    fn rejects_bad_numbers_and_unknown_flags() {
        assert!(parse_str("--port nope").is_err());
        assert!(parse_str("--timeout 0").is_err());
        assert!(parse_str("--nonsense").is_err());
        assert!(parse_str("--host").is_err());
    }

    #[test]
    fn help_is_returned_as_a_message() {
        let err = parse_str("--help").unwrap_err();
        assert!(matches!(err, CliError::Help(_)));
        assert!(err.message().contains("USAGE"));
        assert_eq!(err.exit_code(), 0);
        assert_eq!(parse_str("--nope").unwrap_err().exit_code(), 2);
    }
}

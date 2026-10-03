//! The command line: `eludite-dbg-netfx [--listen HOST:PORT]`.

/// The default listen address: loopback, any free port. The transport has no authentication or encryption
/// (brief 0004 leaves both out), so binding beyond loopback is an explicit choice.
pub const DEFAULT_LISTEN: &str = "127.0.0.1:0";

pub const USAGE: &str = "usage: eludite-dbg-netfx [--listen HOST:PORT]

Serves one DAP client over TCP. The default, 127.0.0.1:0, listens on loopback on a free port;
the chosen address is printed to stderr as 'listening on HOST:PORT'. The transport is neither
authenticated nor encrypted: bind to a non-loopback address only on a trusted network.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cli {
    Listen(String),
    Help,
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Cli, String> {
    let mut listen = DEFAULT_LISTEN.to_string();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--help" | "-h" => return Ok(Cli::Help),
            "--listen" => {
                listen = it.next().ok_or("--listen needs HOST:PORT")?;
            }
            s if s.starts_with("--listen=") => listen = s["--listen=".len()..].to_string(),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    if !listen.contains(':') {
        return Err(format!("--listen {listen:?} is not HOST:PORT"));
    }
    Ok(Cli::Listen(listen))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> Result<Cli, String> {
        parse(a.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_listen() {
        assert_eq!(p(&[]).unwrap(), Cli::Listen("127.0.0.1:0".into()));
        assert_eq!(
            p(&["--listen", "0.0.0.0:4711"]).unwrap(),
            Cli::Listen("0.0.0.0:4711".into())
        );
        assert_eq!(
            p(&["--listen=[::1]:9"]).unwrap(),
            Cli::Listen("[::1]:9".into())
        );
        assert_eq!(p(&["--help"]).unwrap(), Cli::Help);
        assert!(p(&["--listen"]).is_err());
        assert!(p(&["--listen", "4711"]).is_err());
        assert!(p(&["--port", "1"]).is_err());
    }
}

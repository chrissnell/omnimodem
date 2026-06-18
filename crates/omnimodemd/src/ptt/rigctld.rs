//! Hamlib `rigctld` PTT over TCP. Portable on every OS (pure std::net). The
//! line protocol: `T 1`/`T 0` sets PTT and replies `RPRT <n>` (0 = ok); `t`
//! gets PTT and replies a bare `0`/`1` line (NOT followed by RPRT) on success.
//! Lifted from Graywolf `tx/ptt_rigctld.rs`, mapped to structured `PttError`
//! (no `-9999` sentinel — improvement over Graywolf).

use super::{PttDriver, PttError};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

const IO_TIMEOUT: Duration = Duration::from_millis(500);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const UNKEY_SAFETY_RETRIES: u32 = 3;

/// Parse an `RPRT <n>` reply. `Ok(())` on `RPRT 0`; otherwise a structured
/// error. A malformed line is an `Io` error (no magic sentinel).
pub fn parse_rprt(line: &str) -> Result<(), PttError> {
    let line = line.trim();
    match line.strip_prefix("RPRT ") {
        Some(code) => match code.trim().parse::<i32>() {
            Ok(0) => Ok(()),
            Ok(n) => Err(PttError::Io(format!("rigctld RPRT {n}"))),
            Err(_) => Err(PttError::Io(format!("malformed rigctld reply: {line:?}"))),
        },
        None => Err(PttError::Io(format!("expected RPRT, got: {line:?}"))),
    }
}

/// Parse the bare `t` (get-PTT) reply: a single `0` or `1` line. An error
/// surfaces as `RPRT <n>` instead, which `parse_rprt` turns into an error.
pub fn parse_get_ptt(line: &str) -> Result<bool, PttError> {
    match line.trim() {
        "1" => Ok(true),
        "0" => Ok(false),
        other => parse_rprt(other).map(|_| false),
    }
}

/// A rigctld connection. `key`/`unkey` send `T 1`/`T 0`. Unkey is safety-retried
/// (a stuck-keyed radio is worse than a failed key); Drop force-unkeys.
pub struct RigctldPtt {
    stream: TcpStream,
    reader: BufReader<TcpStream>,
    addr: String,
}

impl RigctldPtt {
    pub fn connect(addr: &str) -> Result<Self, PttError> {
        let sockaddr = addr
            .to_socket_addrs()
            .ok()
            .and_then(|mut it| it.next())
            .ok_or_else(|| PttError::Config(format!("unresolvable rigctld addr {addr}")))?;
        let stream = TcpStream::connect_timeout(&sockaddr, CONNECT_TIMEOUT)
            .map_err(|e| map_io(addr, e))?;
        stream.set_read_timeout(Some(IO_TIMEOUT)).ok();
        stream.set_write_timeout(Some(IO_TIMEOUT)).ok();
        stream.set_nodelay(true).ok();
        let reader = BufReader::new(stream.try_clone().map_err(|e| map_io(addr, e))?);
        let mut d = RigctldPtt { stream, reader, addr: addr.to_string() };
        // Startup-unkey, parity with the other drivers (never start keyed).
        d.unkey()?;
        Ok(d)
    }

    fn command_rprt(&mut self, cmd: &str) -> Result<(), PttError> {
        self.stream.write_all(cmd.as_bytes()).map_err(|e| map_io(&self.addr, e))?;
        let mut line = String::new();
        self.reader.read_line(&mut line).map_err(|e| map_io(&self.addr, e))?;
        parse_rprt(&line)
    }
}

impl PttDriver for RigctldPtt {
    fn key(&mut self) -> Result<(), PttError> {
        self.command_rprt("T 1\n")
    }
    fn unkey(&mut self) -> Result<(), PttError> {
        let mut last = self.command_rprt("T 0\n");
        let mut tries = 0;
        while last.is_err() && tries < UNKEY_SAFETY_RETRIES {
            std::thread::sleep(Duration::from_millis(150));
            last = self.command_rprt("T 0\n");
            tries += 1;
        }
        last
    }
}

impl Drop for RigctldPtt {
    fn drop(&mut self) {
        let _ = self.command_rprt("T 0\n"); // never leave a rig keyed
    }
}

/// Map a socket io error to a structured `PttError`.
fn map_io(addr: &str, e: std::io::Error) -> PttError {
    use std::io::ErrorKind::*;
    match e.kind() {
        PermissionDenied => PttError::PermissionDenied { device: addr.into() },
        ConnectionRefused | ConnectionReset | NotConnected | BrokenPipe => {
            PttError::DeviceGone { device: addr.into() }
        }
        _ => PttError::Io(format!("{addr}: {e}")),
    }
}

#[cfg(test)]
mod parse_tests {
    use super::*;

    #[test]
    fn rprt_zero_is_ok() {
        assert!(parse_rprt("RPRT 0").is_ok());
        assert!(parse_rprt("RPRT 0\n").is_ok());
    }

    #[test]
    fn rprt_nonzero_is_err() {
        assert!(matches!(parse_rprt("RPRT -1"), Err(PttError::Io(_))));
    }

    #[test]
    fn malformed_is_err_not_sentinel() {
        assert!(matches!(parse_rprt("garbage"), Err(PttError::Io(_))));
        assert!(matches!(parse_rprt(""), Err(PttError::Io(_))));
    }

    #[test]
    fn get_ptt_parses_bare_line() {
        assert_eq!(parse_get_ptt("1").unwrap(), true);
        assert_eq!(parse_get_ptt("0").unwrap(), false);
        assert!(parse_get_ptt("RPRT -5").is_err());
    }
}

#[cfg(test)]
mod server_tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    /// A minimal rigctld stand-in: answers `T 0/1` with `RPRT 0` and records keys.
    pub(crate) fn fake_rigctld() -> (String, mpsc::Receiver<bool>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            if let Ok((sock, _)) = listener.accept() {
                let mut w = sock.try_clone().unwrap();
                let mut r = BufReader::new(sock);
                let mut line = String::new();
                while r.read_line(&mut line).unwrap_or(0) > 0 {
                    if let Some(rest) = line.trim().strip_prefix("T ") {
                        let _ = tx.send(rest == "1");
                    }
                    let _ = w.write_all(b"RPRT 0\n");
                    line.clear();
                }
            }
        });
        (addr, rx)
    }

    #[test]
    fn connect_keys_and_unkeys() {
        let (addr, rx) = fake_rigctld();
        let mut d = RigctldPtt::connect(&addr).unwrap();
        assert_eq!(rx.recv().unwrap(), false); // startup unkey
        d.key().unwrap();
        assert_eq!(rx.recv().unwrap(), true);
        d.unkey().unwrap();
        assert_eq!(rx.recv().unwrap(), false);
    }
}

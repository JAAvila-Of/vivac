//! The security gate, as a pure function over headers.
//!
//! `d149`: this does not know a socket exists. `admit` takes the handful of
//! headers a request carries and returns a verdict; nothing here binds a
//! port, reads a stream or writes a response. That is what lets every denial
//! in `WEB.md` §4.1 be a unit test instead of a test that first has to stand
//! up a server.
//!
//! The realistic attacker is not outside the machine -- nothing listens
//! anywhere but `127.0.0.1` -- it is a page already open in the same
//! browser as the vivac tab. `admit` is written against that attacker.

use crate::failure::Failure;

/// One session's worth of secrets, born with the port they belong to.
pub struct Gate {
    port: u16,
    token: String,
    /// The one-time key that unlocks the boot page. `None` once spent, and a
    /// spent key never lights up again -- see `admit` below.
    boot: Option<String>,
}

/// The headers `admit` needs, lifted out of whatever carried them in.
pub struct Incoming<'a> {
    /// With the query string, exactly as it arrived.
    pub path: &'a str,
    pub host: Option<&'a str>,
    pub origin: Option<&'a str>,
    /// The value of `X-Vivac-Token`.
    pub token: Option<&'a str>,
}

pub enum Verdict {
    /// Serve the boot page, which carries the session token inside it.
    Boot,
    /// A legitimate request from a page that already holds the token.
    Serve,
    Deny(Denial),
}

/// Why a request was refused.
///
/// `NoValidToken` covers three different reasons on purpose, and they are
/// not told apart: no token at all, the wrong token, and a boot key already
/// spent. Distinguishing them would give a page probing for a valid token
/// three different answers instead of one, which is an oracle -- and the
/// safest oracle is the one that cannot be built because the type has
/// nowhere to carry the difference.
pub enum Denial {
    ForeignHost,
    ForeignOrigin,
    NoValidToken,
}

fn random_hex(bytes: usize) -> Result<String, Failure> {
    let mut buf = vec![0u8; bytes];
    // `getrandom::Error` does not implement `std::error::Error` under the
    // default features this crate builds with, so its message travels
    // across as a plain string instead of the error value itself.
    getrandom::getrandom(&mut buf)
        .map_err(|e| Failure::Io(std::io::Error::other(e.to_string())))?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// Constant-time equality: every byte of both strings is visited and no
/// branch depends on where they first differ. A page cannot read the body of
/// a cross-origin response -- CORS already stops that -- but it can time
/// one, and an early return on the first mismatched byte is exactly the
/// signal a timing attack against a token needs.
///
/// The length mismatch is folded into the same accumulator rather than
/// checked with a short-circuiting `!=` up front, so a guess of the wrong
/// length costs the same as one of the right length.
fn constant_time_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    let mut diff: u8 = (a.len() != b.len()) as u8;
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= x ^ y;
    }
    diff == 0
}

/// The value of the `k` query parameter, if any. No percent-decoding: the
/// key is hex, which never needs decoding, and a decoder here would be
/// parsing surface in the one path security is watching.
fn boot_key(path: &str) -> Option<&str> {
    let query = path.split_once('?').map(|(_, q)| q).unwrap_or("");
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == "k").then_some(v)
    })
}

impl Gate {
    pub fn new(port: u16) -> Result<Gate, Failure> {
        Ok(Gate {
            port,
            token: random_hex(32)?,
            boot: Some(random_hex(32)?),
        })
    }

    /// The URL that carries the boot key. Read once, right after `new`,
    /// before anything has had a chance to spend the key it names.
    pub fn boot_url(&self) -> String {
        format!(
            "http://127.0.0.1:{}/?k={}",
            self.port,
            self.boot.as_deref().unwrap_or("")
        )
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// The order below is part of the contract, not an implementation
    /// detail: `Host` first, because DNS rebinding arrives with a foreign
    /// `Host` and has to die before anything looks at the path or compares a
    /// token.
    pub fn admit(&mut self, r: &Incoming) -> Verdict {
        let host_ok = match r.host {
            Some(h) => {
                h == format!("127.0.0.1:{}", self.port) || h == format!("localhost:{}", self.port)
            }
            None => false,
        };
        if !host_ok {
            return Verdict::Deny(Denial::ForeignHost);
        }

        if let Some(origin) = r.origin {
            let origin_ok = origin == format!("http://127.0.0.1:{}", self.port)
                || origin == format!("http://localhost:{}", self.port);
            if !origin_ok {
                return Verdict::Deny(Denial::ForeignOrigin);
            }
        }

        if let (Some(key), Some(expected)) = (boot_key(r.path), self.boot.as_deref()) {
            if constant_time_eq(key, expected) {
                // Spent. It does not light up again.
                self.boot = None;
                return Verdict::Boot;
            }
        }

        if let Some(token) = r.token {
            if constant_time_eq(token, &self.token) {
                return Verdict::Serve;
            }
        }

        Verdict::Deny(Denial::NoValidToken)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PORT: u16 = 4173;

    fn gate() -> Gate {
        Gate::new(PORT).unwrap_or_else(|e| panic!("{}", e.message()))
    }

    fn incoming<'a>(
        path: &'a str,
        host: Option<&'a str>,
        origin: Option<&'a str>,
        token: Option<&'a str>,
    ) -> Incoming<'a> {
        Incoming {
            path,
            host,
            origin,
            token,
        }
    }

    fn is_denied(v: Verdict, want: fn(&Denial) -> bool) -> bool {
        matches!(v, Verdict::Deny(d) if want(&d))
    }

    fn is_foreign_host(v: Verdict) -> bool {
        is_denied(v, |d| matches!(d, Denial::ForeignHost))
    }

    fn is_foreign_origin(v: Verdict) -> bool {
        is_denied(v, |d| matches!(d, Denial::ForeignOrigin))
    }

    fn is_no_valid_token(v: Verdict) -> bool {
        is_denied(v, |d| matches!(d, Denial::NoValidToken))
    }

    #[test]
    fn the_loopback_host_passes_by_ip_and_by_name() {
        let mut g = gate();
        let token = g.token().to_string();
        let ip_host = format!("127.0.0.1:{PORT}");
        assert!(matches!(
            g.admit(&incoming("/", Some(&ip_host), None, Some(&token))),
            Verdict::Serve
        ));
        let name_host = format!("localhost:{PORT}");
        assert!(matches!(
            g.admit(&incoming("/", Some(&name_host), None, Some(&token))),
            Verdict::Serve
        ));
    }

    #[test]
    fn a_foreign_host_is_denied() {
        let mut g = gate();
        assert!(is_foreign_host(g.admit(&incoming(
            "/",
            Some("malo.com"),
            None,
            None
        ))));
    }

    #[test]
    fn the_right_name_on_the_wrong_port_is_a_foreign_host() {
        let mut g = gate();
        assert!(is_foreign_host(g.admit(&incoming(
            "/",
            Some("127.0.0.1:9999"),
            None,
            None
        ))));
    }

    #[test]
    fn no_host_header_at_all_is_a_foreign_host() {
        let mut g = gate();
        assert!(is_foreign_host(g.admit(&incoming("/", None, None, None))));
    }

    #[test]
    fn no_origin_with_a_good_token_serves() {
        let mut g = gate();
        let token = g.token().to_string();
        let host = format!("127.0.0.1:{PORT}");
        assert!(matches!(
            g.admit(&incoming("/", Some(&host), None, Some(&token))),
            Verdict::Serve
        ));
    }

    #[test]
    fn a_foreign_origin_is_denied() {
        let mut g = gate();
        let token = g.token().to_string();
        let host = format!("127.0.0.1:{PORT}");
        assert!(is_foreign_origin(g.admit(&incoming(
            "/",
            Some(&host),
            Some("http://malo.com"),
            Some(&token)
        ))));
    }

    #[test]
    fn a_matching_origin_with_a_good_token_serves() {
        let mut g = gate();
        let token = g.token().to_string();
        let host = format!("127.0.0.1:{PORT}");
        let origin = format!("http://127.0.0.1:{PORT}");
        assert!(matches!(
            g.admit(&incoming("/", Some(&host), Some(&origin), Some(&token))),
            Verdict::Serve
        ));
    }

    /// The order is the point: a foreign `Host` is refused before a good
    /// token could ever redeem it.
    #[test]
    fn a_foreign_host_stays_denied_even_with_a_good_token() {
        let mut g = gate();
        let token = g.token().to_string();
        assert!(is_foreign_host(g.admit(&incoming(
            "/",
            Some("malo.com"),
            None,
            Some(&token)
        ))));
    }

    #[test]
    fn the_boot_key_works_once_and_never_again() {
        let mut g = gate();
        let host = format!("127.0.0.1:{PORT}");
        let key = g.boot.clone().unwrap();
        let path = format!("/?k={key}");
        assert!(matches!(
            g.admit(&incoming(&path, Some(&host), None, None)),
            Verdict::Boot
        ));
        // The same key, offered again, no longer unlocks anything.
        assert!(is_no_valid_token(g.admit(&incoming(
            &path,
            Some(&host),
            None,
            None
        ))));
    }

    #[test]
    fn no_token_and_no_key_is_denied() {
        let mut g = gate();
        let host = format!("127.0.0.1:{PORT}");
        assert!(is_no_valid_token(g.admit(&incoming(
            "/",
            Some(&host),
            None,
            None
        ))));
    }

    #[test]
    fn a_wrong_token_is_denied() {
        let mut g = gate();
        let host = format!("127.0.0.1:{PORT}");
        assert!(is_no_valid_token(g.admit(&incoming(
            "/",
            Some(&host),
            None,
            Some("not-the-token")
        ))));
    }

    #[test]
    fn a_wrong_boot_key_is_denied() {
        let mut g = gate();
        let host = format!("127.0.0.1:{PORT}");
        assert!(is_no_valid_token(g.admit(&incoming(
            "/?k=not-the-key",
            Some(&host),
            None,
            None
        ))));
    }

    /// Spending the boot key does not touch the session token: the two are
    /// independent secrets.
    #[test]
    fn the_session_token_still_serves_after_the_boot_key_is_spent() {
        let mut g = gate();
        let host = format!("127.0.0.1:{PORT}");
        let key = g.boot.clone().unwrap();
        let path = format!("/?k={key}");
        g.admit(&incoming(&path, Some(&host), None, None));
        let token = g.token().to_string();
        assert!(matches!(
            g.admit(&incoming("/", Some(&host), None, Some(&token))),
            Verdict::Serve
        ));
    }

    #[test]
    fn two_gates_never_share_a_token() {
        let a = gate();
        let b = gate();
        assert_ne!(a.token(), b.token());
    }
}

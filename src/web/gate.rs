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
    /// The one-time key that starts a session. `None` once spent, and a
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
    /// The raw `Cookie` header, exactly as it arrived. `d190`: this is how
    /// a browser carries the token, because following a link sends no
    /// header a page chose.
    pub cookie: Option<&'a str>,
}

pub enum Verdict {
    /// Spend the boot key: hand the session token over as a cookie and
    /// send the browser to the front page (`d190`). It used to serve a page
    /// that carried the token in a `<meta>` nothing ever read, which is
    /// what `f189` found.
    Boot,
    /// A legitimate request from a page that already holds the token.
    Serve,
    Deny(Denial),
}

/// Why a request was refused.
///
/// `NoValidToken` covers four different reasons on purpose, and they are
/// not told apart: no token at all, the wrong token in the header, the
/// wrong one in the cookie, and a boot key already spent. Distinguishing them would give a page probing for a valid token
/// three different answers instead of one, which is an oracle -- and the
/// safest oracle is the one that cannot be built because the type has
/// nowhere to carry the difference.
pub enum Denial {
    ForeignHost,
    ForeignOrigin,
    NoValidToken,
}

/// The name the session cookie goes by. `d190`.
pub(super) const SESSION_COOKIE: &str = "vivac_session";

/// The session cookie's value out of a raw `Cookie` header, or `None`.
///
/// `d138` refused to hand-write a header parser for anything security
/// watches, and this is the one exception the same reasoning allows: a
/// `Cookie` header is a list of `name=value` separated by `;` and nothing
/// else -- no quoting to get wrong, no continuation lines, no encoding. The
/// name is matched whole after trimming, so `vivac_session_other` is a
/// different cookie and not a prefix of this one, and a malformed jar is
/// simply not a match rather than a case to handle.
fn session_cookie(raw: &str) -> Option<&str> {
    raw.split(';').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name.trim() == SESSION_COOKIE).then(|| value.trim())
    })
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

        // Two doors to the same lock, and the same comparison behind both.
        // The header is how `curl` and the tests speak; the cookie is how a
        // browser does, because following a link sends no header the page
        // chose (`d190`).
        let offered = r.token.or_else(|| r.cookie.and_then(session_cookie));
        if let Some(token) = offered {
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
            cookie: None,
        }
    }

    /// What a browser sends: no `X-Vivac-Token` at all, just the cookie it
    /// was handed when the boot key was spent.
    fn browsing<'a>(path: &'a str, host: &'a str, cookie: &'a str) -> Incoming<'a> {
        Incoming {
            path,
            host: Some(host),
            origin: None,
            token: None,
            cookie: Some(cookie),
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
    /// `f189`: the whole reason this exists. A browser following a link
    /// sends the headers it chooses to send and no header the page asked
    /// for, so the token has to arrive in the one thing it does send back
    /// on its own.
    #[test]
    fn a_browser_carrying_only_the_cookie_is_served() {
        let mut g = gate();
        let token = g.token().to_string();
        let host = format!("127.0.0.1:{PORT}");
        let jar = format!("vivac_session={token}");
        assert!(matches!(
            g.admit(&browsing("/p/x/why/g1", &host, &jar)),
            Verdict::Serve
        ));
    }

    /// A cookie header holds whatever else that origin set, in no
    /// particular order, and the value is read by name and not by position.
    #[test]
    fn the_session_cookie_is_found_among_others() {
        let mut g = gate();
        let token = g.token().to_string();
        let host = format!("127.0.0.1:{PORT}");
        for jar in [
            format!("theme=dark; vivac_session={token}; tz=utc"),
            format!("vivac_session={token}; theme=dark"),
            format!("theme=dark;vivac_session={token}"),
            format!("  vivac_session = {token}  "),
        ] {
            assert!(
                matches!(g.admit(&browsing("/", &host, &jar)), Verdict::Serve),
                "not found in {jar}"
            );
        }
    }

    /// A name the session cookie's own is a prefix of must not be mistaken
    /// for it, in either direction.
    #[test]
    fn a_cookie_whose_name_merely_looks_like_the_session_one_is_refused() {
        let mut g = gate();
        let token = g.token().to_string();
        let host = format!("127.0.0.1:{PORT}");
        for jar in [
            format!("vivac_session_other={token}"),
            format!("not_vivac_session={token}"),
            format!("vivac_sessio={token}"),
        ] {
            assert!(
                is_no_valid_token(g.admit(&browsing("/", &host, &jar))),
                "accepted {jar}"
            );
        }
    }

    #[test]
    fn a_cookie_carrying_the_wrong_token_is_refused() {
        let mut g = gate();
        let host = format!("127.0.0.1:{PORT}");
        assert!(is_no_valid_token(g.admit(&browsing(
            "/",
            &host,
            "vivac_session=0000000000000000000000000000000000000000000000000000000000000000"
        ))));
    }

    /// Malformed jars are not a special case to handle, they are simply not
    /// a match. None of these may panic either.
    #[test]
    fn a_cookie_header_that_makes_no_sense_is_just_not_a_match() {
        let mut g = gate();
        let host = format!("127.0.0.1:{PORT}");
        for jar in ["", ";", "=", "vivac_session", "vivac_session=", "; ;;"] {
            assert!(
                is_no_valid_token(g.admit(&browsing("/", &host, jar))),
                "accepted {jar:?}"
            );
        }
    }

    /// The cookie is a second door, not a replacement: everything the gate
    /// refused before it existed, it still refuses.
    #[test]
    fn a_cookie_does_not_excuse_a_foreign_host_or_origin() {
        let mut g = gate();
        let token = g.token().to_string();
        let jar = format!("vivac_session={token}");
        assert!(is_foreign_host(g.admit(&Incoming {
            path: "/",
            host: Some("evil.example:80"),
            origin: None,
            token: None,
            cookie: Some(&jar),
        })));
        let host = format!("127.0.0.1:{PORT}");
        assert!(is_foreign_origin(g.admit(&Incoming {
            path: "/",
            host: Some(&host),
            origin: Some("http://evil.example"),
            token: None,
            cookie: Some(&jar),
        })));
    }
}

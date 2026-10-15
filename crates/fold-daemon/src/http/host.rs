use std::net::IpAddr;

use axum::extract::{Request, State};
use axum::http::header::{HOST, ORIGIN};
use axum::http::uri::Authority;
use axum::http::{HeaderMap, Method, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use fold_core::{FoldError, codes};

use super::Transport;
use super::error::ApiError;
use crate::daemon::AppState;

/// The layer on every `/api` route of the TCP listener. A DNS-rebinding page reaches the daemon under its own domain name, so a `Host`, or on a write an `Origin`, that names anything but this listener is refused with `host_not_allowed`. A request without the header comes from a client that is not a browser and passes; so does a request over the Unix socket.
pub async fn require_listen_host(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    if request.extensions().get::<Transport>() == Some(&Transport::Socket) {
        return next.run(request).await;
    }
    match refusal(state.listen, request.method(), request.headers()) {
        None => next.run(request).await,
        Some(header) => ApiError(
            FoldError::new(
                codes::HOST_NOT_ALLOWED,
                format!("the {header} header names a host this daemon does not listen on"),
            )
            .with_hint("open the web interface by the address the daemon listens on"),
        )
        .into_response(),
    }
}

/// The header that names a host outside `listen`, if any.
fn refusal(listen: IpAddr, method: &Method, headers: &HeaderMap) -> Option<&'static str> {
    if let Some(host) = headers.get(HOST) {
        let allowed = host
            .to_str()
            .ok()
            .and_then(|text| text.parse::<Authority>().ok())
            .is_some_and(|authority| host_allowed(listen, authority.host()));
        if !allowed {
            return Some("Host");
        }
    }
    let write = !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS);
    if let Some(origin) = headers.get(ORIGIN).filter(|_| write) {
        let allowed = origin
            .to_str()
            .ok()
            .and_then(|text| text.parse::<Uri>().ok())
            .filter(|uri| matches!(uri.scheme_str(), Some("http" | "https")))
            .is_some_and(|uri| uri.host().is_some_and(|host| host_allowed(listen, host)));
        if !allowed {
            return Some("Origin");
        }
    }
    None
}

/// `localhost`, or an IP literal this listener can be reached at: any loopback address on loopback, the bound address on a specific one, any address on `0.0.0.0` or `::`. A domain name other than `localhost` is refused, because that is what a rebinding page uses. The port is not compared: the Vite dev server forwards its own `Host` and `Origin`.
pub fn host_allowed(listen: IpAddr, host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let literal = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host);
    let Ok(ip) = literal.parse::<IpAddr>() else {
        return false;
    };
    if listen.is_loopback() {
        ip.is_loopback()
    } else {
        listen.is_unspecified() || ip == listen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn loopback_takes_localhost_and_loopback_literals_only() {
        let listen = ip("127.0.0.1");
        for host in ["localhost", "LocalHost", "127.0.0.1", "127.0.0.2", "[::1]"] {
            assert!(host_allowed(listen, host), "{host}");
        }
        for host in [
            "evil.example",
            "localhost.evil.example",
            "192.168.1.5",
            "[::2]",
            "[::1",
            "",
        ] {
            assert!(!host_allowed(listen, host), "{host}");
        }
    }

    #[test]
    fn a_specific_address_takes_itself_and_every_address_takes_any_literal() {
        let specific = ip("192.168.1.5");
        assert!(host_allowed(specific, "192.168.1.5"));
        assert!(host_allowed(specific, "localhost"));
        assert!(!host_allowed(specific, "192.168.1.6"));
        assert!(!host_allowed(specific, "box.lan"));
        for listen in [ip("0.0.0.0"), ip("::")] {
            assert!(host_allowed(listen, "10.0.0.7"));
            assert!(host_allowed(listen, "[fe80::1]"));
            assert!(!host_allowed(listen, "evil.example"));
        }
    }

    fn headers(pairs: &[(axum::http::HeaderName, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(name.clone(), value.parse().unwrap());
        }
        map
    }

    #[test]
    fn the_origin_counts_on_a_write_only() {
        let listen = ip("127.0.0.1");
        let evil = headers(&[(HOST, "127.0.0.1:7420"), (ORIGIN, "http://evil.example")]);
        assert_eq!(refusal(listen, &Method::GET, &evil), None);
        assert_eq!(refusal(listen, &Method::PUT, &evil), Some("Origin"));
        let null = headers(&[(ORIGIN, "null")]);
        assert_eq!(refusal(listen, &Method::POST, &null), Some("Origin"));
        let file = headers(&[(ORIGIN, "file://localhost")]);
        assert_eq!(refusal(listen, &Method::POST, &file), Some("Origin"));
        let dev = headers(&[(HOST, "localhost:5173"), (ORIGIN, "http://localhost:5173")]);
        assert_eq!(refusal(listen, &Method::DELETE, &dev), None);
        let bad_host = headers(&[(HOST, "evil.example:7420")]);
        assert_eq!(refusal(listen, &Method::GET, &bad_host), Some("Host"));
        assert_eq!(refusal(listen, &Method::GET, &HeaderMap::new()), None);
    }
}

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

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

/// The header the web client sends on every request. A page of another origin, such as an app on another localhost port, can send a form or a bodiless request without a CORS preflight, but not a custom header; the daemon answers no preflight, so a write without the header did not come from the web client.
pub const CLIENT_HEADER: &str = "x-fold-client";

/// The layer on every `/api` route of the TCP listener. A DNS-rebinding page reaches the daemon under its own domain name, so a `Host`, or on a write an `Origin`, that names anything but this listener is refused with `host_not_allowed`. A request without those headers comes from a client that is not a browser and passes that check; so does a request over the Unix socket. A write without [`CLIENT_HEADER`] is refused with `client_header_required`.
pub async fn require_listen_host(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    if request.extensions().get::<Transport>() == Some(&Transport::Socket) {
        return next.run(request).await;
    }
    if is_write(request.method()) && !request.headers().contains_key(CLIENT_HEADER) {
        return ApiError(
            FoldError::new(
                codes::CLIENT_HEADER_REQUIRED,
                format!("a write needs the {CLIENT_HEADER} header"),
            )
            .with_hint("send the request from the fold web interface"),
        )
        .into_response();
    }
    match refusal(state.listen.ip(), request.method(), request.headers()) {
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
    if let Some(origin) = headers.get(ORIGIN).filter(|_| is_write(method)) {
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

fn is_write(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
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

/// The origin a browser on this machine opens for a listener bound to `listen`: a listener on every address, `0.0.0.0` or `::`, answers on loopback too, while a browser cannot open the unspecified address itself.
pub fn web_origin(listen: SocketAddr) -> String {
    let reachable = match listen {
        SocketAddr::V4(addr) if addr.ip().is_unspecified() => {
            SocketAddr::from((Ipv4Addr::LOCALHOST, addr.port()))
        }
        SocketAddr::V6(addr) if addr.ip().is_unspecified() => {
            SocketAddr::from((Ipv6Addr::LOCALHOST, addr.port()))
        }
        addr => addr,
    };
    format!("http://{reachable}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_web_origin_names_loopback_for_every_address_and_the_address_otherwise() {
        for (listen, origin) in [
            ("0.0.0.0:7420", "http://127.0.0.1:7420"),
            ("[::]:7420", "http://[::1]:7420"),
            ("127.0.0.1:7420", "http://127.0.0.1:7420"),
            ("192.168.1.5:7420", "http://192.168.1.5:7420"),
            ("[::1]:9000", "http://[::1]:9000"),
        ] {
            assert_eq!(web_origin(listen.parse().unwrap()), origin, "{listen}");
        }
    }

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

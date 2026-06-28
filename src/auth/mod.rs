//! Server-mode authentication: accounts, password hashing and opaque session
//! tokens. The whole module is inert in local mode — identity there is the
//! implicit `local-admin` (see [`crate::server::identity`]).
//!
//! A request carries its session token either as `Authorization: Bearer <token>`
//! (API clients) or a `session=<token>` cookie (the browser SPA); both resolve
//! through [`AuthService::authenticate`]. The ownership/admin *rules* still live
//! in `server::identity` (`scope`, `assert_admin`) — this module only decides
//! *who* the caller is.

mod password;
pub mod routes;
mod service;

pub use routes::router;
pub use service::{AuthService, AuthServiceError, Authenticated};

use axum::http::{header, HeaderMap};

/// Name of the browser session cookie set on login/register.
const SESSION_COOKIE: &str = "session";

/// Pull a session token off a request: `Authorization: Bearer <token>` first,
/// then a `session=<token>` cookie. Returns `None` if neither is present.
pub fn token_from_headers(headers: &HeaderMap) -> Option<String> {
    if let Some(token) = bearer_token(headers) {
        return Some(token);
    }
    cookie_token(headers)
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let token = value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))?;
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_string())
}

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    let cookies = headers.get(header::COOKIE)?.to_str().ok()?;
    cookies.split(';').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name.trim() == SESSION_COOKIE).then(|| value.trim().to_string())
    })
}

#[cfg(test)]
mod token_tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn reads_bearer_token() {
        let h = headers(&[("authorization", "Bearer abc123")]);
        assert_eq!(token_from_headers(&h).as_deref(), Some("abc123"));
    }

    #[test]
    fn reads_session_cookie() {
        let h = headers(&[("cookie", "foo=bar; session=tok42; baz=qux")]);
        assert_eq!(token_from_headers(&h).as_deref(), Some("tok42"));
    }

    #[test]
    fn bearer_wins_over_cookie() {
        let h = headers(&[("authorization", "Bearer hdr"), ("cookie", "session=cke")]);
        assert_eq!(token_from_headers(&h).as_deref(), Some("hdr"));
    }

    #[test]
    fn none_without_credentials() {
        assert_eq!(token_from_headers(&headers(&[])), None);
        assert_eq!(
            token_from_headers(&headers(&[("authorization", "Bearer ")])),
            None
        );
    }
}

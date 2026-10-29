//! First run: a server without any account lets the computer it runs on create the first admin
//! (`GET /setup`, `POST /setup`), so no password has to go through the launcher or a command
//! line. Allowed only while there are no accounts, and only to a direct connection from the
//! loopback address: a request that came through a proxy (an `X-Forwarded-For` or `Forwarded`
//! header) is refused even when the proxy runs on the same computer.

use std::net::SocketAddr;

use axum::{
    Json,
    extract::{ConnectInfo, FromRequestParts, State},
    http::{HeaderMap, StatusCode, request::Parts},
};
use poe_accounts::{Role, TokenKind, password_digest};
use tokio::sync::Mutex;

use crate::{
    ApiError, WebState,
    dto::{LoginResponse, RegisterRequest, SetupStatus},
    error::ApiJson,
    public::{blocking, check_password, issue},
};

/// The connection's peer address, when the server records it (it does: `into_make_service_with_connect_info`).
pub struct Peer(pub Option<SocketAddr>);

impl<S: Send + Sync> FromRequestParts<S> for Peer {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|c| c.0),
        ))
    }
}

/// One setup at a time, so two requests cannot both find the server empty.
static SETUP: Mutex<()> = Mutex::const_new(());

/// A direct request from this computer.
pub fn is_local(peer: Option<SocketAddr>, headers: &HeaderMap) -> bool {
    peer.is_some_and(|p| p.ip().is_loopback())
        && !headers.contains_key("x-forwarded-for")
        && !headers.contains_key("forwarded")
}

async fn no_accounts(s: &WebState) -> Result<bool, ApiError> {
    let accounts = s.accounts.clone();
    Ok(blocking(move || accounts.list()).await??.is_empty())
}

/// Whether the server still needs its first admin, and whether this request may create it.
#[utoipa::path(get, path = "/api/v1/setup", tag = "public",
    responses((status = 200, body = SetupStatus)))]
pub async fn status(
    State(s): State<WebState>,
    Peer(peer): Peer,
    headers: HeaderMap,
) -> Result<Json<SetupStatus>, ApiError> {
    let local = is_local(peer, &headers);
    // Only this computer learns whether the server has accounts.
    let needed = local && no_accounts(&s).await?;
    Ok(Json(SetupStatus {
        needed,
        local,
        open_registration: s.config.accounts.open_registration,
        name: s.config.gateway.display_name.clone(),
    }))
}

/// Creates the first account, with the admin role, and signs it in (a `web` token, as
/// `POST /login` returns). `403` once any account exists or when the request is not a direct
/// one from this computer.
#[utoipa::path(post, path = "/api/v1/setup", tag = "public", request_body = RegisterRequest,
    responses((status = 201, body = LoginResponse), (status = 400, body = crate::ErrorBody),
        (status = 403, description = "the server has accounts already, or the request is not from this computer", body = crate::ErrorBody)))]
pub async fn create(
    State(s): State<WebState>,
    Peer(peer): Peer,
    headers: HeaderMap,
    ApiJson(req): ApiJson<RegisterRequest>,
) -> Result<(StatusCode, Json<LoginResponse>), ApiError> {
    if !is_local(peer, &headers) {
        return Err(ApiError::forbidden(
            "the first admin can only be created on the computer the server runs on",
        ));
    }
    check_password(&req.password)?;
    let _one = SETUP.lock().await;
    if !no_accounts(&s).await? {
        return Err(ApiError::forbidden(
            "the server has accounts already: sign in, or ask an admin",
        ));
    }
    let accounts = s.accounts.clone();
    let account = blocking(move || {
        accounts.create_account(&req.username, &password_digest(&req.password), Role::Admin)
    })
    .await??;
    tracing::info!(account = %account.username, "first admin created through the web admin setup");
    Ok((
        StatusCode::CREATED,
        Json(issue(&s, account, TokenKind::Web).await?),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_direct_loopback_requests_are_local() {
        let lo = Some("127.0.0.1:5000".parse().unwrap());
        let lo6 = Some("[::1]:5000".parse().unwrap());
        let lan = Some("192.168.1.5:5000".parse().unwrap());
        let none = HeaderMap::new();
        assert!(is_local(lo, &none));
        assert!(is_local(lo6, &none));
        assert!(!is_local(lan, &none));
        assert!(!is_local(None, &none), "an unknown peer is not local");
        let mut proxied = HeaderMap::new();
        proxied.insert("x-forwarded-for", "203.0.113.9".parse().unwrap());
        assert!(!is_local(lo, &proxied));
        let mut fwd = HeaderMap::new();
        fwd.insert("forwarded", "for=203.0.113.9".parse().unwrap());
        assert!(!is_local(lo, &fwd));
    }
}

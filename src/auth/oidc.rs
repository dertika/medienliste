//! OpenID Connect (Authorization Code Flow mit PKCE).

use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use openidconnect::core::{CoreAuthenticationFlow, CoreClient, CoreGenderClaim, CoreProviderMetadata};
use openidconnect::{
    AdditionalClaims, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointMaybeSet, EndpointNotSet,
    EndpointSet, IssuerUrl, Nonce, OAuth2TokenResponse, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl,
    Scope, TokenResponse, UserInfoClaims,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use url::Url;

use crate::config::OidcConfig;

type Client = CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

/// Zusätzliche Claims von Authelia (Gruppen).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct GroupsClaims {
    #[serde(default)]
    groups: Vec<String>,
}
impl AdditionalClaims for GroupsClaims {}

/// Die für die Anmeldung relevanten Angaben über den Benutzer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginClaims {
    pub sub: String,
    pub name: String,
    pub email: String,
    pub groups: Vec<String>,
}

pub struct AuthRequest {
    pub url: Url,
    pub state: String,
    pub pkce_verifier: String,
    pub nonce: String,
}

/// Kapselt Discovery (lazy, mit Cache) und den Code-Austausch.
pub struct OidcService {
    cfg: OidcConfig,
    http: openidconnect::reqwest::Client,
    client: RwLock<Option<(Instant, Client)>>,
}

const DISCOVERY_TTL: Duration = Duration::from_secs(3600);

impl OidcService {
    pub fn new(cfg: OidcConfig) -> Result<Self> {
        let http = openidconnect::reqwest::ClientBuilder::new()
            // Wichtig gegen SSRF: keine Weiterleitungen folgen.
            .redirect(openidconnect::reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .user_agent(crate::USER_AGENT)
            .build()?;
        Ok(Self { cfg, http, client: RwLock::new(None) })
    }

    async fn client(&self) -> Result<Client> {
        if let Some((at, c)) = self.client.read().await.as_ref()
            && at.elapsed() < DISCOVERY_TTL
        {
            return Ok(c.clone());
        }
        let issuer = IssuerUrl::new(self.cfg.issuer.clone()).context("ungültiger OIDC-Issuer")?;
        let meta = CoreProviderMetadata::discover_async(issuer, &self.http)
            .await
            .map_err(|e| anyhow!("OIDC-Discovery fehlgeschlagen: {e}"))?;
        let client = CoreClient::from_provider_metadata(
            meta,
            ClientId::new(self.cfg.client_id.clone()),
            self.cfg.client_secret.clone().map(ClientSecret::new),
        )
        .set_redirect_uri(RedirectUrl::new(self.cfg.redirect_url.clone())?);
        *self.client.write().await = Some((Instant::now(), client.clone()));
        Ok(client)
    }

    /// Erzeugt die Autorisierungs-URL samt PKCE-Verifier, State und Nonce.
    pub async fn begin(&self) -> Result<AuthRequest> {
        let client = self.client().await?;
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let mut req = client.authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        );
        for s in self.cfg.scopes.iter().filter(|s| s.as_str() != "openid") {
            req = req.add_scope(Scope::new(s.clone()));
        }
        let (url, state, nonce) = req.set_pkce_challenge(challenge).url();
        Ok(AuthRequest {
            url,
            state: state.secret().clone(),
            pkce_verifier: verifier.secret().clone(),
            nonce: nonce.secret().clone(),
        })
    }

    /// Tauscht den Code gegen Tokens, prüft das ID-Token und liest die Userinfo.
    pub async fn finish(&self, code: &str, pkce_verifier: &str, nonce: &str) -> Result<LoginClaims> {
        let client = self.client().await?;
        let token = client
            .exchange_code(AuthorizationCode::new(code.to_string()))?
            .set_pkce_verifier(PkceCodeVerifier::new(pkce_verifier.to_string()))
            .request_async(&self.http)
            .await
            .map_err(|e| anyhow!("Token-Austausch fehlgeschlagen: {e}"))?;

        let id_token = token.id_token().ok_or_else(|| anyhow!("kein ID-Token erhalten"))?;
        let verifier = client.id_token_verifier();
        let claims = id_token
            .claims(&verifier, &Nonce::new(nonce.to_string()))
            .map_err(|e| anyhow!("ID-Token ungültig: {e}"))?;

        let sub = claims.subject().to_string();
        let mut name = claims
            .name()
            .and_then(|n| n.get(None))
            .map(|n| n.to_string())
            .or_else(|| claims.preferred_username().map(|u| u.to_string()));
        let mut email = claims.email().map(|e| e.to_string());
        let mut groups = Vec::new();

        // Authelia liefert Gruppen (und je nach Claims-Policy auch Name/E-Mail) nur über Userinfo.
        if let Ok(req) = client.user_info(token.access_token().clone(), Some(claims.subject().clone())) {
            let info: Result<UserInfoClaims<GroupsClaims, CoreGenderClaim>, _> =
                req.request_async(&self.http).await;
            match info {
                Ok(info) => {
                    if name.is_none() {
                        name = info
                            .name()
                            .and_then(|n| n.get(None))
                            .map(|n| n.to_string())
                            .or_else(|| info.preferred_username().map(|u| u.to_string()));
                    }
                    if email.is_none() {
                        email = info.email().map(|e| e.to_string());
                    }
                    groups = info.additional_claims().groups.clone();
                }
                Err(e) => tracing::warn!("Userinfo konnte nicht gelesen werden: {e}"),
            }
        }

        if sub.is_empty() {
            bail!("ID-Token ohne sub-Claim");
        }
        Ok(LoginClaims { sub, name: name.unwrap_or_default(), email: email.unwrap_or_default(), groups })
    }

    pub fn is_admin(&self, claims: &LoginClaims) -> bool {
        match &self.cfg.admin_group {
            Some(g) => claims.groups.iter().any(|x| x == g),
            None => false,
        }
    }

    /// Darf sich der Benutzer anmelden? Ist eine Pflichtgruppe konfiguriert, muss er
    /// Mitglied dieser Gruppe oder der Admin-Gruppe sein. Ohne Gruppen-Claim
    /// (z. B. weil Userinfo nicht lesbar war) wird der Zugang verweigert.
    pub fn is_allowed(&self, claims: &LoginClaims) -> bool {
        match &self.cfg.required_group {
            Some(g) => claims.groups.iter().any(|x| x == g) || self.is_admin(claims),
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(required: Option<&str>, admin: Option<&str>) -> OidcService {
        OidcService::new(OidcConfig {
            issuer: "https://idp.invalid".into(),
            client_id: "x".into(),
            client_secret: None,
            redirect_url: "https://app.invalid/auth/callback".into(),
            scopes: vec!["openid".into()],
            admin_group: admin.map(String::from),
            required_group: required.map(String::from),
        })
        .unwrap()
    }

    fn claims(groups: &[&str]) -> LoginClaims {
        LoginClaims {
            sub: "s".into(),
            name: String::new(),
            email: String::new(),
            groups: groups.iter().map(|g| g.to_string()).collect(),
        }
    }

    #[test]
    fn required_group() {
        let open = service(None, Some("admins"));
        assert!(open.is_allowed(&claims(&[])));

        let s = service(Some("medienliste"), Some("admins"));
        assert!(s.is_allowed(&claims(&["medienliste"])));
        assert!(s.is_allowed(&claims(&["admins"])), "Admins dürfen immer");
        assert!(!s.is_allowed(&claims(&["andere"])));
        assert!(!s.is_allowed(&claims(&[])));
    }
}

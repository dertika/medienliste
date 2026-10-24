//! Login-Flow gegen einen Fake-OIDC-Provider (wiremock):
//! PKCE, State/Nonce, ID-Token-Prüfung, Just-in-time-Provisioning und Admin-Gruppe.

mod common;

use axum::http::{StatusCode, header};
use chrono::{Duration, Utc};
use openidconnect::core::{
    CoreIdToken, CoreIdTokenClaims, CoreJwsSigningAlgorithm, CoreRsaPrivateSigningKey,
};
use openidconnect::{
    Audience, EmptyAdditionalClaims, EndUserEmail, EndUserName, IssuerUrl, JsonWebKeyId, LocalizedClaim,
    Nonce, PrivateSigningKey, StandardClaims, SubjectIdentifier,
};
use wiremock::matchers::{body_string_contains, header_exists, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::*;

const KEY: &str = include_str!("fixtures/oidc_test_key.pem");

fn signing_key() -> CoreRsaPrivateSigningKey {
    CoreRsaPrivateSigningKey::from_pem(KEY, Some(JsonWebKeyId::new("test".into()))).unwrap()
}

async fn fake_idp() -> MockServer {
    let server = MockServer::start().await;
    let issuer = server.uri();
    let discovery = serde_json::json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/authorize"),
        "token_endpoint": format!("{issuer}/token"),
        "userinfo_endpoint": format!("{issuer}/userinfo"),
        "jwks_uri": format!("{issuer}/jwks"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256"],
        "code_challenge_methods_supported": ["S256"]
    });
    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(discovery))
        .mount(&server)
        .await;
    let jwks = serde_json::json!({ "keys": [signing_key().as_verification_key()] });
    Mock::given(method("GET"))
        .and(path("/jwks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(jwks))
        .mount(&server)
        .await;
    server
}

fn id_token(issuer: &str, sub: &str, name: &str, nonce: &str, aud: &str) -> String {
    let mut name_claim = LocalizedClaim::new();
    name_claim.insert(None, EndUserName::new(name.into()));
    let claims = CoreIdTokenClaims::new(
        IssuerUrl::new(issuer.into()).unwrap(),
        vec![Audience::new(aud.into())],
        Utc::now() + Duration::minutes(5),
        Utc::now(),
        StandardClaims::new(SubjectIdentifier::new(sub.into()))
            .set_name(Some(name_claim))
            .set_email(Some(EndUserEmail::new(format!("{sub}@example.org")))),
        EmptyAdditionalClaims {},
    )
    .set_nonce(Some(Nonce::new(nonce.into())));
    CoreIdToken::new(claims, &signing_key(), CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256, None, None)
        .unwrap()
        .to_string()
}

/// Startet den Login und gibt (state, nonce, Location) zurück.
async fn begin_login(h: &Harness) -> (String, String, String) {
    let (st, headers, _) = h.get("/auth/login?return_to=liste%3Ftyp%3Dbook", None).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    let location = headers[header::LOCATION].to_str().unwrap().to_string();
    let url = url::Url::parse(&location).unwrap();
    let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    let state = q["state"].clone();
    let nonce: String = sqlx::query_scalar("SELECT nonce FROM login_states WHERE state = ?1")
        .bind(&state)
        .fetch_one(&h.pool)
        .await
        .unwrap();
    (state, nonce, location)
}

#[tokio::test]
async fn login_redirect_uses_pkce() {
    let idp = fake_idp().await;
    let h = harness_with(&idp.uri(), default_providers()).await;
    let (_, _, location) = begin_login(&h).await;
    assert!(location.starts_with(&format!("{}/authorize?", idp.uri())));
    assert!(location.contains("response_type=code"));
    assert!(location.contains("code_challenge_method=S256"));
    assert!(location.contains("code_challenge="));
    assert!(location.contains("nonce="));
    assert!(location.contains("client_id=medienliste"));
    assert!(location.contains("groups"));
    assert!(location.contains(&urlencoding::encode("https://medienliste.test/auth/callback").to_string()));
}

#[tokio::test]
async fn full_login_provisions_user_and_updates_claims() {
    let idp = fake_idp().await;
    let h = harness_with(&idp.uri(), default_providers()).await;

    for (round, name, groups) in
        [(1, "Erika Muster", vec!["medienliste-admins"]), (2, "Erika Neu", vec!["medienliste-users"])]
    {
        let (state, nonce, _) = begin_login(&h).await;
        let token = id_token(&idp.uri(), "sub-erika", name, &nonce, "medienliste");
        idp.reset().await;
        // reset entfernt alle Mocks → Discovery wird gecacht, Token/Userinfo neu anlegen.
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("code_verifier="))
            .and(body_string_contains("code=code-123"))
            .and(header_exists("authorization"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "at-123", "token_type": "Bearer", "expires_in": 3600, "id_token": token
            })))
            .expect(1)
            .mount(&idp)
            .await;
        Mock::given(method("GET"))
            .and(path("/userinfo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "sub": "sub-erika", "groups": groups
            })))
            .mount(&idp)
            .await;

        let (st, headers, _) = h.get(&format!("/auth/callback?code=code-123&state={state}"), None).await;
        assert_eq!(st, StatusCode::SEE_OTHER, "Runde {round}");
        assert_eq!(headers[header::LOCATION], "/liste?typ=book");
        let cookie = headers[header::SET_COOKIE].to_str().unwrap();
        assert!(cookie.contains("HttpOnly") && cookie.contains("Secure") && cookie.contains("SameSite=Lax"));

        let (n, db_name, email, admin): (i64, String, String, bool) = sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM users), name, email, is_admin FROM users WHERE sub = 'sub-erika'",
        )
        .fetch_one(&h.pool)
        .await
        .unwrap();
        assert_eq!(n, 1, "kein Duplikat beim zweiten Login");
        assert_eq!(db_name, name);
        assert_eq!(email, "sub-erika@example.org");
        assert_eq!(admin, round == 1, "Admin-Rolle folgt der Gruppe");

        // Mit dem neuen Cookie ist man angemeldet.
        let session = cookie.split(';').next().unwrap().to_string();
        let u = TestUser { id: 0, cookie: session, csrf: String::new() };
        let (st, _, body) = h.get("/liste", Some(&u)).await;
        assert_eq!(st, StatusCode::OK);
        assert!(body.contains(name));

        // State ist einmalig verwendbar.
        let (st, _, _) = h.get(&format!("/auth/callback?code=code-123&state={state}"), None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn wrong_nonce_or_audience_is_rejected() {
    let idp = fake_idp().await;
    let h = harness_with(&idp.uri(), default_providers()).await;

    for token_fn in [
        |iss: &str, _nonce: &str| id_token(iss, "sub-x", "X", "falsche-nonce", "medienliste"),
        |iss: &str, nonce: &str| id_token(iss, "sub-x", "X", nonce, "anderer-client"),
    ] {
        let (state, nonce, _) = begin_login(&h).await;
        let token = token_fn(&idp.uri(), &nonce);
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "at", "token_type": "Bearer", "id_token": token
            })))
            .up_to_n_times(1)
            .mount(&idp)
            .await;
        let (st, headers, _) = h.get(&format!("/auth/callback?code=c&state={state}"), None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(headers.get(header::SET_COOKIE).is_none());
    }
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users").fetch_one(&h.pool).await.unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn unknown_state_is_rejected() {
    let idp = fake_idp().await;
    let h = harness_with(&idp.uri(), default_providers()).await;
    let (st, headers, body) = h.get("/auth/callback?code=c&state=erfunden", None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(headers.get(header::SET_COOKIE).is_none());
    assert!(body.contains("Anmeldung fehlgeschlagen"));
}

#[tokio::test]
async fn logout_redirects_to_configured_url() {
    let h = harness_with_config("https://idp.invalid", default_providers(), |c| {
        c.logout_url = Some("https://example.org/".parse().unwrap());
    })
    .await;
    let a = h.user("user-a", false).await;
    let (st, headers, _) = h.post("/auth/logout", Some(&a), &[]).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(headers[header::LOCATION], "https://example.org/");
    let csp = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
    assert!(csp.contains("form-action 'self' https://example.org"), "{csp}");
    assert!(headers[header::SET_COOKIE].to_str().unwrap().contains("Max-Age=0"));
    assert_eq!(h.get("/liste", Some(&a)).await.0, StatusCode::SEE_OTHER, "Sitzung muss beendet sein");
}

#[tokio::test]
async fn logout_deletes_session() {
    let h = harness().await;
    let a = h.user("user-a", false).await;
    let (st, headers, _) = h.post("/auth/logout", Some(&a), &[]).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(headers[header::LOCATION], "/auth/abgemeldet", "ohne Logout-URL eigene Seite");
    assert!(headers[header::SET_COOKIE].to_str().unwrap().contains("Max-Age=0"));
    let (st, _, _) = h.get("/liste", Some(&a)).await;
    assert_eq!(st, StatusCode::SEE_OTHER, "Sitzung muss ungültig sein");
}

/// Mockt Token- und Userinfo-Endpoint für einen Login mit den angegebenen Gruppen.
async fn mount_login(idp: &MockServer, sub: &str, nonce: &str, groups: &[&str]) {
    let token = id_token(&idp.uri(), sub, "Max Fremd", nonce, "medienliste");
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "at", "token_type": "Bearer", "id_token": token
        })))
        .up_to_n_times(1)
        .mount(idp)
        .await;
    Mock::given(method("GET"))
        .and(path("/userinfo"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({ "sub": sub, "groups": groups })),
        )
        .up_to_n_times(1)
        .mount(idp)
        .await;
}

#[tokio::test]
async fn login_without_required_group_creates_no_user() {
    let idp = fake_idp().await;
    let h = harness_with(&idp.uri(), default_providers()).await;
    let (state, nonce, _) = begin_login(&h).await;
    mount_login(&idp, "sub-fremd", &nonce, &["andere-gruppe"]).await;

    let (st, headers, body) = h.get(&format!("/auth/callback?code=c&state={state}"), None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert!(headers.get(header::SET_COOKIE).is_none());
    assert!(body.contains("nicht freigeschaltet"));
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users").fetch_one(&h.pool).await.unwrap();
    assert_eq!(n, 0, "Benutzer ohne Pflichtgruppe darf nicht angelegt werden");
}

#[tokio::test]
async fn removed_from_group_ends_existing_sessions() {
    let idp = fake_idp().await;
    let h = harness_with(&idp.uri(), default_providers()).await;
    // Bestehender Benutzer mit gültiger Sitzung.
    let u = h.user("sub-ex", false).await;
    assert_eq!(h.get("/liste", Some(&u)).await.0, StatusCode::OK);
    let name_before: String =
        sqlx::query_scalar("SELECT name FROM users WHERE sub = 'sub-ex'").fetch_one(&h.pool).await.unwrap();

    // Erneuter Login, inzwischen ohne Gruppe.
    let (state, nonce, _) = begin_login(&h).await;
    mount_login(&idp, "sub-ex", &nonce, &[]).await;
    let (st, _, _) = h.get(&format!("/auth/callback?code=c&state={state}"), None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Alte Sitzung ist beendet, Stammdaten unverändert.
    assert_eq!(h.get("/liste", Some(&u)).await.0, StatusCode::SEE_OTHER);
    let name_after: String =
        sqlx::query_scalar("SELECT name FROM users WHERE sub = 'sub-ex'").fetch_one(&h.pool).await.unwrap();
    assert_eq!(name_before, name_after);
}

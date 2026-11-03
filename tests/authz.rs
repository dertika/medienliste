//! Berechtigungstests: Benutzer A darf nie Daten von Benutzer B sehen oder ändern.

mod common;

use axum::http::{StatusCode, header};
use common::*;
use medienliste::models::{MediaType, NewMedia, Status};
use medienliste::repo::entries;

/// A hat „Der Schwarm“ gelesen, B möchte „Momo“ lesen.
async fn setup() -> (Harness, TestUser, TestUser, i64, i64) {
    let h = harness().await;
    let a = h.user("user-a", false).await;
    let b = h.user("user-b", false).await;
    let schwarm = h.media(&book_hit("Der Schwarm", "b1", "9783596164530"), Some(a.id)).await;
    let momo = h.media(&book_hit("Momo", "b3", "9783522202107"), Some(b.id)).await;
    entries::set_status(
        &h.pool,
        a.id,
        schwarm,
        Status::Done,
        chrono::NaiveDate::from_ymd_opt(2024, 5, 1).unwrap(),
    )
    .await
    .unwrap();
    entries::set_status(
        &h.pool,
        b.id,
        momo,
        Status::Wanted,
        chrono::NaiveDate::from_ymd_opt(2024, 5, 1).unwrap(),
    )
    .await
    .unwrap();
    (h, a, b, schwarm, momo)
}

#[tokio::test]
async fn list_shows_only_own_entries() {
    let (h, a, b, _, _) = setup().await;

    let (st, _, body) = h.get("/liste", Some(&a)).await;
    assert_eq!(st, StatusCode::OK);
    assert!(body.contains("Der Schwarm"));
    assert!(!body.contains("Momo"), "A sieht Eintrag von B");
    assert!(body.contains("1 Eintrag"));

    let (_, _, body) = h.get("/liste", Some(&b)).await;
    assert!(body.contains("Momo"));
    assert!(!body.contains("Der Schwarm"), "B sieht Eintrag von A");

    // Auch mit Filtern und Textsuche keine fremden Einträge.
    let (_, _, body) = h.get("/liste?q=schwarm&status=done&typ=book", Some(&b)).await;
    assert!(!body.contains("Der Schwarm"));
    assert!(body.contains("0 Einträge"));
}

#[tokio::test]
async fn detail_shows_only_own_status() {
    let (h, a, b, schwarm, _) = setup().await;

    let (st, _, body) = h.get(&format!("/medium/{schwarm}"), Some(&a)).await;
    assert_eq!(st, StatusCode::OK);
    assert!(body.contains("badge-done"));

    // Medien sind global sichtbar, der Status von A aber nicht.
    let (st, _, body) = h.get(&format!("/medium/{schwarm}"), Some(&b)).await;
    assert_eq!(st, StatusCode::OK);
    assert!(body.contains("Der Schwarm"));
    assert!(!body.contains("badge-done"), "B sieht den Status von A");
    assert!(body.contains("Nicht in deiner Liste"));
}

#[tokio::test]
async fn foreign_media_id_only_touches_own_entry() {
    let (h, a, b, schwarm, _) = setup().await;
    let before = h.entry_status(a.id, schwarm).await.unwrap();

    // B setzt einen Status für das Medium aus A's Liste → eigener Eintrag für B.
    let (st, _, _) = h.post(&format!("/medium/{schwarm}/status"), Some(&b), &[("status", "wanted")]).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(h.entry_status(b.id, schwarm).await.unwrap().0, "wanted");
    assert_eq!(h.entry_status(a.id, schwarm).await.unwrap(), before, "Eintrag von A wurde verändert");

    // B entfernt „seinen“ Eintrag → A's Eintrag bleibt.
    let (st, _, _) = h.post(&format!("/medium/{schwarm}/entfernen"), Some(&b), &[]).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert!(h.entry_status(b.id, schwarm).await.is_none());
    assert_eq!(h.entry_status(a.id, schwarm).await.unwrap(), before);
}

#[tokio::test]
async fn user_id_in_form_is_ignored() {
    let (h, a, b, schwarm, _) = setup().await;
    let a_id = a.id.to_string();
    let before = h.entry_status(a.id, schwarm).await.unwrap();
    h.post(
        &format!("/medium/{schwarm}/status"),
        Some(&b),
        &[("status", "wanted"), ("user_id", &a_id), ("user", &a_id)],
    )
    .await;
    assert_eq!(h.entry_status(a.id, schwarm).await.unwrap(), before);
}

#[tokio::test]
async fn stats_are_per_user() {
    let (h, a, b, _, _) = setup().await;
    let (_, _, body) = h.get("/statistik", Some(&a)).await;
    assert!(body.contains(r#"<span class="num">1</span><span>📖 Bücher gelesen"#), "{body}");

    let (_, _, body) = h.get("/statistik", Some(&b)).await;
    assert!(body.contains(r#"<span class="num">0</span><span>📖 Bücher gelesen"#));
    assert!(body.contains(r#"<span class="num">1</span><span>☆ Bücher auf der Wunschliste"#));
}

#[tokio::test]
async fn search_badges_show_only_own_status() {
    let (h, a, b, _, _) = setup().await;

    let (_, _, body) = h.get("/suche?typ=book&q=schwarm", Some(&a)).await;
    assert!(body.contains("badge-done"), "A sieht eigenen Status nicht");
    assert!(body.contains("Gelesen"));

    let (_, _, body) = h.get("/suche?typ=book&q=schwarm", Some(&b)).await;
    assert!(body.contains("Der Schwarm"));
    assert!(!body.contains("badge-done"), "B sieht Status von A in der Suche");
    assert!(body.contains("Nicht in deiner Liste"));
}

#[tokio::test]
async fn csrf_token_of_other_user_is_rejected() {
    let (h, a, b, schwarm, _) = setup().await;
    let before = h.entry_status(a.id, schwarm).await.unwrap();

    // Cookie von A, aber CSRF-Token von B.
    let (st, _, _) = h
        .post(&format!("/medium/{schwarm}/status"), Some(&a), &[("status", "wanted"), ("csrf", &b.csrf)])
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Ohne Token.
    let pairs = vec![("status".to_string(), "wanted".to_string())];
    let (st, _, _) =
        h.post_raw(&format!("/medium/{schwarm}/status"), Some(&a.cookie), Some(ORIGIN), &pairs, false).await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Fremder Origin trotz gültigem Token.
    let pairs = vec![("status".to_string(), "wanted".to_string()), ("csrf".to_string(), a.csrf.clone())];
    let (st, _, _) = h
        .post_raw(
            &format!("/medium/{schwarm}/status"),
            Some(&a.cookie),
            Some("https://evil.example"),
            &pairs,
            false,
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    assert_eq!(h.entry_status(a.id, schwarm).await.unwrap(), before);
}

#[tokio::test]
async fn unauthenticated_access_is_denied() {
    let (h, _, _, schwarm, _) = setup().await;
    for path in ["/liste", "/suche?q=schwarm", "/statistik", "/admin", &format!("/medium/{schwarm}"), "/neu"]
    {
        let (st, headers, body) = h.get(path, None).await;
        assert_eq!(st, StatusCode::SEE_OTHER, "{path}");
        assert!(headers[header::LOCATION].to_str().unwrap().starts_with("/auth/login?return_to="));
        assert!(!body.contains("Der Schwarm"));
    }
    let (st, _, _) = h.post(&format!("/medium/{schwarm}/status"), None, &[("status", "done")]).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // Ungültiges Cookie.
    let fake = TestUser { id: 0, cookie: "medienliste_session=ungueltig".into(), csrf: String::new() };
    let (st, _, _) = h.get("/liste", Some(&fake)).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn health_endpoints_need_no_login() {
    let h = harness().await;
    assert_eq!(h.get("/healthz", None).await.0, StatusCode::OK);
    assert_eq!(h.get("/readyz", None).await.0, StatusCode::OK);
    assert_eq!(h.get("/static/app.css", None).await.0, StatusCode::OK);
    let (st, _, body) = h.get("/manifest.webmanifest", None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(body.contains("\"Medienliste\""));
}

#[tokio::test]
async fn manual_media_edit_only_by_creator_or_admin() {
    let h = harness().await;
    let a = h.user("user-a", false).await;
    let b = h.user("user-b", false).await;
    let admin = h.user("admin", true).await;
    let manual = NewMedia {
        media_type: Some(MediaType::Book),
        title: "Selbst erfasst".into(),
        source: "manual".into(),
        ..Default::default()
    };
    let id = h.media(&manual, Some(a.id)).await;

    assert_eq!(h.get(&format!("/medium/{id}/bearbeiten"), Some(&a)).await.0, StatusCode::OK);
    assert_eq!(h.get(&format!("/medium/{id}/bearbeiten"), Some(&b)).await.0, StatusCode::FORBIDDEN);
    assert_eq!(h.get(&format!("/medium/{id}/bearbeiten"), Some(&admin)).await.0, StatusCode::OK);

    // Löschen nur durch Admins.
    assert_eq!(h.post(&format!("/medium/{id}/loeschen"), Some(&a), &[]).await.0, StatusCode::FORBIDDEN);
    assert_eq!(h.post(&format!("/medium/{id}/loeschen"), Some(&admin), &[]).await.0, StatusCode::SEE_OTHER);
    assert_eq!(h.get(&format!("/medium/{id}"), Some(&a)).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn admin_page_hides_personal_lists() {
    let (h, a, _, _, _) = setup().await;
    let admin = h.user("admin", true).await;
    assert_eq!(h.get("/admin", Some(&a)).await.0, StatusCode::FORBIDDEN);
    let (st, _, body) = h.get("/admin", Some(&admin)).await;
    assert_eq!(st, StatusCode::OK);
    assert!(body.contains("Name user-a"));
    assert!(!body.contains("Der Schwarm"));
    assert!(!body.contains("Momo"));

    // Auch Admins sehen in ihrer Liste nur eigene Einträge.
    let (_, _, body) = h.get("/liste", Some(&admin)).await;
    assert!(!body.contains("Der Schwarm") && !body.contains("Momo"));
}

#[tokio::test]
async fn add_from_search_creates_shared_media_and_private_entries() {
    let h = harness().await;
    let a = h.user("user-a", false).await;
    let b = h.user("user-b", false).await;

    let (st, _, body) = h
        .post_htmx(
            "/suche/hinzufuegen",
            &a,
            &[("source", "stubaudio"), ("external_id", "B004V0L2VG"), ("status", "done")],
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert!(body.contains("Gehört"), "{body}");

    // B fügt dasselbe Hörbuch hinzu → gleiches Medium, eigener Eintrag.
    h.post(
        "/suche/hinzufuegen",
        Some(&b),
        &[("source", "stubaudio"), ("external_id", "B004V0L2VG"), ("status", "wanted")],
    )
    .await;
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM media").fetch_one(&h.pool).await.unwrap();
    assert_eq!(n, 1);
    let id: i64 = sqlx::query_scalar("SELECT id FROM media").fetch_one(&h.pool).await.unwrap();
    assert_eq!(h.entry_status(a.id, id).await.unwrap().0, "done");
    assert_eq!(h.entry_status(b.id, id).await.unwrap().0, "wanted");

    // Hörbuch-Suche zeigt jedem seinen Status.
    let (_, _, body) = h.get_htmx("/suche?typ=audiobook&q=schwarm", &b).await;
    assert!(body.contains("Möchte ich hören"));
    assert!(body.contains("badge-wanted"));
    assert!(!body.contains("badge-done"));
    // htmx bekommt nur das Teilergebnis.
    assert!(!body.contains("<html"));

    // Unbekannte Quelle/ID → 404, kein Medium aus Client-Daten.
    let (st, _, _) = h
        .post(
            "/suche/hinzufuegen",
            Some(&a),
            &[("source", "stubbook"), ("external_id", "erfunden"), ("status", "done")],
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn status_change_date_only_updates_on_change() {
    let (h, a, _, schwarm, _) = setup().await;
    let before = h.entry_status(a.id, schwarm).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    h.post(&format!("/medium/{schwarm}/status"), Some(&a), &[("status", "done")]).await;
    assert_eq!(h.entry_status(a.id, schwarm).await.unwrap(), before);
    h.post(&format!("/medium/{schwarm}/status"), Some(&a), &[("status", "wanted")]).await;
    let after = h.entry_status(a.id, schwarm).await.unwrap();
    assert_eq!(after.0, "wanted");
    assert_ne!(after.1, before.1);
}

#[tokio::test]
async fn open_redirect_is_prevented() {
    let (h, a, _, schwarm, _) = setup().await;
    let (st, headers, _) = h
        .post(
            &format!("/medium/{schwarm}/status"),
            Some(&a),
            &[("status", "done"), ("return_to", "//evil.example/x")],
        )
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(headers[header::LOCATION], format!("/medium/{schwarm}").as_str());
}

fn multipart(fields: &[(&str, &str)], file: Option<&[u8]>) -> (String, Vec<u8>) {
    let boundary = "----medienlistetest";
    let mut body = Vec::new();
    for (k, v) in fields {
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n").as_bytes(),
        );
    }
    if let Some(f) = file {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"cover\"; filename=\"c.png\"\r\nContent-Type: image/png\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(f);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

#[tokio::test]
async fn manual_create_with_cover_and_validation() {
    use axum::body::Body;
    use axum::http::Request;
    let h = harness().await;
    let a = h.user("user-a", false).await;
    let b = h.user("user-b", false).await;
    let png: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3];

    let send = |fields: Vec<(&'static str, String)>, file: Option<&'static [u8]>, cookie: String| {
        let f: Vec<(&str, &str)> = fields.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let (ct, body) = multipart(&f, file);
        Request::post("/neu")
            .header(header::CONTENT_TYPE, ct)
            .header(header::COOKIE, cookie)
            .header(header::ORIGIN, ORIGIN)
            .body(Body::from(body))
            .unwrap()
    };

    // Ungültige ISBN → Formular mit Fehlermeldung.
    let req = send(
        vec![
            ("csrf", a.csrf.clone()),
            ("media_type", "book".into()),
            ("title", "X".into()),
            ("isbn", "123".into()),
        ],
        None,
        a.cookie.clone(),
    );
    let (st, _, body) = h.request(req).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body.contains("10 oder 13 Stellen"));

    // Gültig, mit Cover, Status „gehört“.
    let png_static: &'static [u8] = Box::leak(png.to_vec().into_boxed_slice());
    let req = send(
        vec![
            ("csrf", a.csrf.clone()),
            ("media_type", "audiobook".into()),
            ("title", "Mein Hörspiel".into()),
            ("authors", "Autorin Eins, Autor Zwei".into()),
            ("narrators", "Sprecherin".into()),
            ("status", "done".into()),
        ],
        Some(png_static),
        a.cookie.clone(),
    );
    let (st, headers, _) = h.request(req).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    let loc = headers[header::LOCATION].to_str().unwrap().to_string();
    let id: i64 = loc.trim_start_matches("/medium/").parse().unwrap();
    assert_eq!(h.entry_status(a.id, id).await.unwrap().0, "done");
    let (_, _, body) = h.get(&loc, Some(&a)).await;
    assert!(body.contains("Gehört"));
    assert!(body.contains("Autorin Eins, Autor Zwei"));
    let cover: String = sqlx::query_scalar("SELECT cover_file FROM media WHERE id = ?1")
        .bind(id)
        .fetch_one(&h.pool)
        .await
        .unwrap();
    let (st, headers, _) = h.get(&format!("/covers/{cover}"), Some(&a)).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "image/png");
    assert_eq!(h.get(&format!("/covers/{cover}"), None).await.0, StatusCode::SEE_OTHER);

    // B darf das manuelle Medium nicht bearbeiten (auch nicht per POST).
    let mut req = send(
        vec![("csrf", b.csrf.clone()), ("media_type", "book".into()), ("title", "Gekapert".into())],
        None,
        b.cookie.clone(),
    );
    *req.uri_mut() = format!("/medium/{id}/bearbeiten").parse().unwrap();
    assert_eq!(h.request(req).await.0, StatusCode::FORBIDDEN);
    let title: String = sqlx::query_scalar("SELECT title FROM media WHERE id = ?1")
        .bind(id)
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(title, "Mein Hörspiel");
    // B hat keinen Eintrag bekommen.
    assert!(h.entry_status(b.id, id).await.is_none());
}

#[tokio::test]
async fn htmx_history_restore_gets_full_page() {
    use axum::body::Body;
    use axum::http::Request;
    let h = harness().await;
    let a = h.user("user-a", false).await;
    let req = Request::get("/suche?typ=book&q=momo")
        .header(header::COOKIE, &a.cookie)
        .header("hx-request", "true")
        .header("hx-history-restore-request", "true")
        .body(Body::empty())
        .unwrap();
    let (st, _, body) = h.request(req).await;
    assert_eq!(st, StatusCode::OK);
    assert!(body.contains("<html"));
    assert!(body.contains("Momo"));
}

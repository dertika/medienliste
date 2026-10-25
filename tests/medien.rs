//! Filme und das Jahr „gelesen/gehört/gesehen“.

mod common;

use std::sync::Arc;

use axum::http::{StatusCode, header};
use chrono::Datelike;
use common::*;
use medienliste::models::MediaType;
use medienliste::providers::Provider;

fn this_year() -> i32 {
    chrono::Utc::now().with_timezone(&chrono_tz::Europe::Berlin).year()
}

async fn done_year(h: &Harness, user_id: i64, media_id: i64) -> Option<i32> {
    sqlx::query_scalar("SELECT done_year FROM user_media WHERE user_id = ?1 AND media_id = ?2")
        .bind(user_id)
        .bind(media_id)
        .fetch_one(&h.pool)
        .await
        .unwrap()
}

async fn add_movie(h: &Harness, u: &TestUser, status: &str) -> i64 {
    let (st, _, body) = h
        .post_htmx(
            "/suche/hinzufuegen",
            u,
            &[("source", "stubmovie"), ("external_id", "104"), ("status", status)],
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    sqlx::query_scalar("SELECT id FROM media WHERE source = 'stubmovie'").fetch_one(&h.pool).await.unwrap()
}

#[tokio::test]
async fn movie_search_and_add_sets_current_year() {
    let h = harness().await;
    let a = h.user("user-a", false).await;

    let (_, _, body) = h.get("/suche?typ=movie&q=lola", Some(&a)).await;
    assert!(body.contains("Lola rennt"));
    assert!(body.contains("Möchte ich sehen"));
    assert!(body.contains("mit Testdarstellerin"));

    let id = add_movie(&h, &a, "done").await;
    assert_eq!(done_year(&h, a.id, id).await, Some(this_year()));
    let (_, _, body) = h.get(&format!("/medium/{id}"), Some(&a)).await;
    assert!(body.contains("Gesehen"));
    assert!(body.contains("Regie: Testregie"));
    assert!(body.contains("Darsteller"));
    assert!(body.contains(&format!("value=\"{}\"", this_year())), "Jahr im Formular vorbelegt");

    // Liste mit Typfilter
    let (_, _, body) = h.get("/liste?typ=movie", Some(&a)).await;
    assert!(body.contains("Lola rennt"));
    assert!(body.contains(&format!("Gesehen {}", this_year())));
    let (_, _, body) = h.get("/liste?typ=book", Some(&a)).await;
    assert!(!body.contains("Lola rennt"));
}

#[tokio::test]
async fn done_year_can_be_edited_and_drives_stats() {
    let h = harness().await;
    let a = h.user("user-a", false).await;
    let id = add_movie(&h, &a, "done").await;

    let (st, _, _) = h.post(&format!("/medium/{id}/jahr"), Some(&a), &[("year", "2015")]).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(done_year(&h, a.id, id).await, Some(2015));
    let (_, _, body) = h.get("/statistik", Some(&a)).await;
    assert!(body.contains("<th scope=\"row\">2015</th>"), "{body}");
    assert!(body.contains(r#"<span class="num">1</span><span>🎬 Filme gesehen"#));

    // Leeres Jahr = unbekannt → eigene Zeile „ohne Jahr“.
    h.post(&format!("/medium/{id}/jahr"), Some(&a), &[("year", "")]).await;
    assert_eq!(done_year(&h, a.id, id).await, None);
    let (_, _, body) = h.get("/statistik", Some(&a)).await;
    assert!(body.contains("ohne Jahr"));

    // Ungültige Jahre werden abgelehnt.
    for bad in ["1800", "abc", &(this_year() + 5).to_string()] {
        let (st, _, _) = h.post(&format!("/medium/{id}/jahr"), Some(&a), &[("year", bad)]).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{bad}");
    }
    assert_eq!(done_year(&h, a.id, id).await, None);
}

#[tokio::test]
async fn status_change_resets_year() {
    let h = harness().await;
    let a = h.user("user-a", false).await;
    let id = add_movie(&h, &a, "done").await;
    h.post(&format!("/medium/{id}/jahr"), Some(&a), &[("year", "2010")]).await;

    // Gleicher Status: Jahr bleibt.
    h.post(&format!("/medium/{id}/status"), Some(&a), &[("status", "done")]).await;
    assert_eq!(done_year(&h, a.id, id).await, Some(2010));
    // Wechsel auf „möchte ich“: Jahr wird entfernt, Jahr-Bearbeitung nicht möglich.
    h.post(&format!("/medium/{id}/status"), Some(&a), &[("status", "wanted")]).await;
    assert_eq!(done_year(&h, a.id, id).await, None);
    let (st, _, _) = h.post(&format!("/medium/{id}/jahr"), Some(&a), &[("year", "2011")]).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    // Wieder „gesehen“: aktuelles Jahr.
    h.post(&format!("/medium/{id}/status"), Some(&a), &[("status", "done")]).await;
    assert_eq!(done_year(&h, a.id, id).await, Some(this_year()));
}

#[tokio::test]
async fn year_of_other_user_cannot_be_changed() {
    let h = harness().await;
    let a = h.user("user-a", false).await;
    let b = h.user("user-b", false).await;
    let id = add_movie(&h, &a, "done").await;
    h.post(&format!("/medium/{id}/jahr"), Some(&a), &[("year", "2012")]).await;

    // B hat keinen Eintrag → 404, A unverändert.
    let (st, _, _) = h.post(&format!("/medium/{id}/jahr"), Some(&b), &[("year", "1999")]).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(done_year(&h, a.id, id).await, Some(2012));

    // B mit eigenem Eintrag ändert nur seinen.
    h.post(&format!("/medium/{id}/status"), Some(&b), &[("status", "done")]).await;
    h.post(&format!("/medium/{id}/jahr"), Some(&b), &[("year", "1999")]).await;
    assert_eq!(done_year(&h, b.id, id).await, Some(1999));
    assert_eq!(done_year(&h, a.id, id).await, Some(2012));

    // Ohne gültiges CSRF-Token nichts.
    let (st, _, _) =
        h.post(&format!("/medium/{id}/jahr"), Some(&a), &[("year", "2000"), ("csrf", "x")]).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(done_year(&h, a.id, id).await, Some(2012));
}

#[tokio::test]
async fn manual_movie_with_year() {
    use axum::body::Body;
    use axum::http::Request;
    let h = harness().await;
    let a = h.user("user-a", false).await;
    let boundary = "----medientest";
    let mut body = String::new();
    for (k, v) in [
        ("csrf", a.csrf.as_str()),
        ("media_type", "movie"),
        ("title", "Der Himmel über Berlin"),
        ("authors", "Wim Wenders"),
        ("narrators", "Bruno Ganz, Otto Sander"),
        ("isbn", "123"), // wird bei Filmen ignoriert
        ("status", "done"),
        ("done_year", "1995"),
    ] {
        body.push_str(&format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n"
        ));
    }
    body.push_str(&format!("--{boundary}--\r\n"));
    let req = Request::post("/neu")
        .header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={boundary}"))
        .header(header::COOKIE, &a.cookie)
        .header(header::ORIGIN, ORIGIN)
        .body(Body::from(body))
        .unwrap();
    let (st, headers, body) = h.request(req).await;
    assert_eq!(st, StatusCode::SEE_OTHER, "{body}");
    let id: i64 = headers[header::LOCATION].to_str().unwrap().trim_start_matches("/medium/").parse().unwrap();
    assert_eq!(done_year(&h, a.id, id).await, Some(1995));
    let (t, isbn): (String, Option<String>) =
        sqlx::query_as("SELECT media_type, isbn13 FROM media WHERE id = ?1")
            .bind(id)
            .fetch_one(&h.pool)
            .await
            .unwrap();
    assert_eq!(t, "movie");
    assert_eq!(isbn, None);
    let (_, _, body) = h.get(&format!("/medium/{id}"), Some(&a)).await;
    assert!(body.contains("Bruno Ganz, Otto Sander"));
}

#[tokio::test]
async fn movie_search_without_provider_shows_hint() {
    let providers: Vec<Arc<dyn Provider>> =
        default_providers().into_iter().filter(|p| p.media_type() != MediaType::Movie).collect();
    let h = harness_with("https://idp.invalid", providers).await;
    let a = h.user("user-a", false).await;
    let (_, _, body) = h.get("/suche?typ=movie&q=lola", Some(&a)).await;
    assert!(body.contains("keine externe Quelle"));
    assert!(body.contains("Manuell anlegen"));
}

#[tokio::test]
async fn movie_series_uses_part_and_assets_are_versioned() {
    let h = harness().await;
    let a = h.user("user-a", false).await;
    let mut m = movie_hit("Die Gefährten", "120");
    m.series = Some("Der Herr der Ringe".into());
    m.series_index = Some("1".into());
    let id = h.media(&m, Some(a.id)).await;
    medienliste::repo::entries::set_status(&h.pool, a.id, id, medienliste::models::Status::Wanted, 2026)
        .await
        .unwrap();

    let (_, _, body) = h.get("/liste", Some(&a)).await;
    assert!(body.contains("Der Herr der Ringe · Teil 1"), "{body}");
    assert!(!body.contains("Band 1"));
    let (_, _, body) = h.get(&format!("/medium/{id}"), Some(&a)).await;
    assert!(body.contains("Filmreihe"));
    assert!(body.contains("· Teil 1"));

    // CSS/JS-URLs tragen eine Inhalts-Prüfsumme, nicht nur die App-Version.
    let v = medienliste::web::asset_version();
    assert_eq!(v.len(), 12);
    assert!(v.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(body.contains(&format!("static/app.css?v={v}")));
}

#[tokio::test]
async fn my_list_is_start_page_and_first_tab() {
    let h = harness().await;
    let a = h.user("user-a", false).await;
    let (st, headers, _) = h.get("/", Some(&a)).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(headers[header::LOCATION], "/liste");

    let (_, _, body) = h.get("/liste", Some(&a)).await;
    let list = body.find("href=\"/liste\"><span class=\"tab-icon\"").expect("Reiter Meine Liste");
    let search = body.find("href=\"/suche\"><span class=\"tab-icon\"").expect("Reiter Suche");
    assert!(list < search, "Meine Liste steht vor der Suche");

    let (_, _, manifest) = h.get("/manifest.webmanifest", None).await;
    assert!(manifest.contains("\"start_url\":\"/liste\""));
}

#[tokio::test]
async fn series_search_add_detail_and_stats() {
    let h = harness().await;
    let a = h.user("user-a", false).await;

    let (_, _, body) = h.get("/suche?typ=series&q=dark", Some(&a)).await;
    assert!(body.contains("Dark"));
    assert!(body.contains("Möchte ich sehen"));
    assert!(body.contains("mit Testdarsteller"));
    assert!(body.contains("value=\"series\" checked"));

    let (st, _, _) = h
        .post_htmx(
            "/suche/hinzufuegen",
            &a,
            &[("source", "stubseries"), ("external_id", "70523"), ("status", "done")],
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let id: i64 = sqlx::query_scalar("SELECT id FROM media WHERE source = 'stubseries'")
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(done_year(&h, a.id, id).await, Some(this_year()));

    let (_, _, body) = h.get(&format!("/medium/{id}"), Some(&a)).await;
    assert!(body.contains("Idee: Testidee"));
    assert!(body.contains("<dt>Sender</dt><dd>Testsender</dd>"));
    assert!(body.contains("Darsteller"));
    assert!(body.contains("Gesehen"));

    let (_, _, body) = h.get("/liste?typ=series", Some(&a)).await;
    assert!(body.contains("Dark"));
    assert!(body.contains("entry series"));

    let (_, _, body) = h.get("/statistik", Some(&a)).await;
    assert!(body.contains(r#"<span class="num">1</span><span>📺 Serien gesehen"#), "{body}");
}

#[tokio::test]
async fn search_hit_opens_preview_without_saving() {
    let h = harness().await;
    let a = h.user("user-a", false).await;
    let b = h.user("user-b", false).await;
    let count = |h: &Harness| {
        let pool = h.pool.clone();
        async move { sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM media").fetch_one(&pool).await.unwrap() }
    };

    let (_, _, body) = h.get("/suche?typ=book&q=schwarm", Some(&a)).await;
    assert!(body.contains(r#"href="/vorschau/stubbook/b1""#), "{body}");

    let (st, _, body) = h.get("/vorschau/stubbook/b1", Some(&a)).await;
    assert_eq!(st, StatusCode::OK);
    assert!(body.contains("<h1>Der Schwarm</h1>"));
    assert!(body.contains("9783596164530"));
    assert!(body.contains(r#"action="/suche/hinzufuegen""#));
    assert!(body.contains(r#"name="show_remove" value="1""#));
    assert!(body.contains("Nicht in deiner Liste"));
    assert_eq!(count(&h).await, 0, "Vorschau speichert nichts");

    // Status aus der Vorschau setzen → weiter zur echten Detailseite.
    let (st, headers, _) = h
        .post_htmx(
            "/suche/hinzufuegen",
            &a,
            &[("source", "stubbook"), ("external_id", "b1"), ("status", "wanted"), ("show_remove", "1")],
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let id: i64 = sqlx::query_scalar("SELECT id FROM media WHERE source = 'stubbook'")
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(headers.get("hx-redirect").unwrap(), &format!("/medium/{id}"));

    // Gespeichert: Vorschau leitet weiter, auch für andere Benutzer (ohne deren Status zu ändern).
    let (st, headers, _) = h.get("/vorschau/stubbook/b1", Some(&b)).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(headers.get(header::LOCATION).unwrap(), &format!("/medium/{id}"));
    assert_eq!(h.entry_status(b.id, id).await, None);

    let (st, _, _) = h.get("/vorschau/unbekannt/b1", Some(&a)).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _, _) = h.get("/vorschau/stubbook/gibtsnicht", Some(&a)).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _, _) = h.get("/vorschau/stubbook/b1", None).await;
    assert_ne!(st, StatusCode::OK, "nur angemeldet");
}

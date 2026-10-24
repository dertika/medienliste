# Medienliste

Selbst gehostete Webapp zum Tracken von **Büchern**, **Hörbüchern** und **Filmen**: suchen, zur eigenen Liste hinzufügen, als „Gelesen/Gehört/Gesehen“ oder „Möchte ich lesen/hören/sehen“ markieren und jederzeit prüfen: *Habe ich das schon?*

- Login ausschließlich über **OpenID Connect** (Authelia, Authorization Code Flow mit PKCE), Benutzer werden beim ersten Login automatisch angelegt
- Strikte Trennung: Jeder Benutzer sieht und bearbeitet nur seine eigenen Einträge
- Metadaten aus **Google Books** (Fallback **Open Library**), dem **Audible-Katalog** (.de) mit **Audnexus** und für Filme aus **TMDB**; Cover werden lokal gecacht
- Pro Eintrag ein **Jahr** („2019 gelesen“), beim Abschließen vorbelegt und änderbar; die Statistik zählt danach
- Manuelles Anlegen, wenn nichts gefunden wird
- Statistik pro Jahr und Typ, Filter nach Typ und Status
- Für das iPhone optimiert, als PWA installierbar („Zum Home-Bildschirm“)
- Ein einzelner, kleiner Container (statisches Rust-Binary auf distroless), läuft rootless unter Podman

## Technik

| Bereich | Umsetzung |
|---|---|
| Backend | Rust, [axum](https://github.com/tokio-rs/axum) |
| Oberfläche | Server-gerendert mit [Askama](https://github.com/askama-rs/askama) (zur Compile-Zeit geprüft), [htmx](https://htmx.org) (lokal ausgeliefert), eigenes CSS |
| Datenbank | SQLite im WAL-Modus über [sqlx](https://github.com/launchbadge/sqlx), versionierte Migrationen in `migrations/` |
| Login | [openidconnect](https://github.com/ramosbugs/openidconnect-rs) |
| Texte | [Fluent](https://projectfluent.org): `locales/de.ftl` |

Alle Assets, Templates, Migrationen und Sprachdateien sind in das Binary eingebettet.

## Umgebungsvariablen

| Variable | Pflicht | Standard | Beschreibung |
|---|---|---|---|
| `MEDIENLISTE_BASE_URL` | ja | – | Öffentliche URL, z. B. `https://medienliste.example.org`. Daraus werden alle absoluten URLs, der Cookie-Pfad, `Secure`-Cookies (bei https) und der erlaubte `Origin` abgeleitet. |
| `MEDIENLISTE_OIDC_ISSUER` | ja | – | Issuer-URL, z. B. `https://auth.example.org` |
| `MEDIENLISTE_OIDC_CLIENT_ID` | ja | – | Client-ID aus der Authelia-Konfiguration |
| `MEDIENLISTE_OIDC_CLIENT_SECRET` | nein | – | Client-Secret (im Klartext). Ohne Secret wird die App als Public Client mit PKCE betrieben. |
| `MEDIENLISTE_LOGOUT_URL` | nein | – | Ziel nach dem Abmelden, z. B. eine Portalseite mit Authelia-Abmeldung oder direkt `https://auth.example.org/logout?rd=https://example.org`. Ohne Angabe erscheint die eigene Abmeldeseite. Siehe [Abmelden](#abmelden). |
| `MEDIENLISTE_OIDC_REDIRECT_URL` | nein | `<BASE_URL>/auth/callback` | Redirect-URI, muss in Authelia eingetragen sein |
| `MEDIENLISTE_OIDC_SCOPES` | nein | `openid profile email groups` | Angefragte Scopes |
| `MEDIENLISTE_OIDC_ADMIN_GROUP` | nein | – | Mitglieder dieser Authelia-Gruppe erhalten die Admin-Rolle (wird bei jedem Login neu ausgewertet) |
| `MEDIENLISTE_OIDC_REQUIRED_GROUP` | nein | – | Wenn gesetzt, dürfen sich nur Mitglieder dieser Gruppe (oder der Admin-Gruppe) anmelden. Andere werden weder angelegt noch aktualisiert, und ihre bestehenden Sitzungen werden beim Anmeldeversuch beendet. |
| `MEDIENLISTE_GOOGLE_BOOKS_API_KEY` | empfohlen | – | API-Key für Google Books. Ohne Key ist das anonyme Kontingent schnell erschöpft; dann springt Open Library ein. |
| `MEDIENLISTE_TMDB_API_KEY` | für Filme | – | API-Key (v3) oder Lese-Token (v4) von [TMDB](https://www.themoviedb.org/settings/api), kostenlos. Ohne Key gibt es keine Filmsuche, Filme lassen sich aber manuell anlegen. |
| `MEDIENLISTE_AUDIBLE_MARKETPLACE` | nein | `de` | Audible-Marktplatz (`de`, `us`, `uk`, `fr`, `it`, `es`, `ca`, `au`, `in`, `jp`) |
| `MEDIENLISTE_DATA_DIR` | nein | `/data` | Datenverzeichnis (Datenbank, Cover, Backups) |
| `MEDIENLISTE_LISTEN` | nein | `0.0.0.0:8080` | Adresse und Port |
| `MEDIENLISTE_TRUSTED_PROXIES` | nein | `127.0.0.0/8,::1/128,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16,fc00::/7` | Nur von diesen Adressen wird `X-Forwarded-For` ausgewertet |
| `MEDIENLISTE_BACKUP_INTERVAL_HOURS` | nein | `24` | Intervall für automatische Backups nach `<DATA_DIR>/backups` (`0` = aus) |
| `MEDIENLISTE_BACKUP_KEEP` | nein | `7` | Anzahl aufbewahrter automatischer Backups |
| `MEDIENLISTE_TIMEZONE` | nein | `Europe/Berlin` | Zeitzone für Datumsanzeige und Jahresstatistik |
| `MEDIENLISTE_SESSION_DAYS` | nein | `30` | Gültigkeit einer Sitzung in Tagen |
| `MEDIENLISTE_LOG` | nein | `info,sqlx=warn` | Log-Filter (`tracing`-Syntax) |

Eine kommentierte Vorlage liegt in [`deploy/medienliste.env.example`](deploy/medienliste.env.example).

## Authelia einrichten

Client in der Authelia-Konfiguration (`identity_providers.oidc.clients`):

```yaml
- client_id: 'medienliste'
  client_name: 'Medienliste'
  # Hash des Secrets: authelia crypto hash generate pbkdf2 --variant sha512
  client_secret: '$pbkdf2-sha512$...'
  public: false
  authorization_policy: 'one_factor'   # oder two_factor
  require_pkce: true
  pkce_challenge_method: 'S256'
  redirect_uris:
    - 'https://medienliste.example.org/auth/callback'
  scopes: ['openid', 'profile', 'email', 'groups']
  response_types: ['code']
  grant_types: ['authorization_code']
  token_endpoint_auth_method: 'client_secret_basic'
  userinfo_signed_response_alg: 'none'
```

Name, E-Mail und Gruppen werden aus dem ID-Token bzw. vom Userinfo-Endpoint gelesen. Benutzer werden anhand des `sub`-Claims zugeordnet, Name und E-Mail werden bei jedem Login aktualisiert.

### Abmelden

„Abmelden“ beendet die Sitzung in der Medienliste. Die Anmeldung beim Identity Provider bleibt aber bestehen (Single Sign-on). Wer danach auf „Anmelden“ tippt, ist deshalb ohne Passwortabfrage sofort wieder drin.

Um auch dort abzumelden, setze `MEDIENLISTE_LOGOUT_URL`, zum Beispiel:

- **Portalseite:** `MEDIENLISTE_LOGOUT_URL=https://example.org`. Von dort meldet man sich bei Authelia ab.
- **Direkt der Authelia-Logout:** `MEDIENLISTE_LOGOUT_URL=https://auth.example.org/logout?rd=https://example.org`. Authelia beendet die Sitzung und leitet zu `rd` weiter. Das Ziel muss in Authelias Session-Domain liegen.

### Zugang auf eine Gruppe beschränken

Mit `MEDIENLISTE_OIDC_REQUIRED_GROUP=medienliste-users` dürfen sich nur Mitglieder dieser Authelia-Gruppe anmelden. Mitglieder der Admin-Gruppe dürfen immer. Für alle anderen gilt:

- Sie sehen die Meldung „nicht freigeschaltet“ (HTTP 403).
- Sie werden **nicht angelegt**, ein bestehender Eintrag wird nicht aktualisiert.
- Ihre noch offenen Sitzungen werden beendet.

Fehlt der Gruppen-Claim, etwa weil der Scope `groups` nicht freigegeben ist, wird der Zugang verweigert.

Die Gruppe wird bei jedem Login geprüft. Wer aus der Gruppe entfernt wird, verliert den Zugang beim nächsten Anmeldeversuch, spätestens aber nach Ablauf der Sitzung (`MEDIENLISTE_SESSION_DAYS`).

Zusätzlich lässt sich der Zugang direkt in Authelia sperren. Dann kommt ein fremder Benutzer gar nicht erst bis zur App:

```yaml
identity_providers:
  oidc:
    authorization_policies:
      medienliste:
        default_policy: 'deny'
        rules:
          - policy: 'one_factor'      # oder two_factor
            subject: 'group:medienliste-users'
          - policy: 'one_factor'
            subject: 'group:medienliste-admins'
    clients:
      - client_id: 'medienliste'
        authorization_policy: 'medienliste'
        # … wie oben
```

## Betrieb mit Podman (rootless, Quadlet)

```sh
# Datenverzeichnis und Konfiguration
mkdir -p ~/medienliste/data ~/.config/medienliste ~/.config/containers/systemd
cp deploy/medienliste.env.example ~/.config/medienliste/medienliste.env
chmod 600 ~/.config/medienliste/medienliste.env    # anpassen!

# Quadlet-Unit installieren und starten
cp deploy/medienliste.container ~/.config/containers/systemd/
systemctl --user daemon-reload
systemctl --user start medienliste
loginctl enable-linger "$USER"                 # Start ohne Login
```

Der Container läuft als UID 65532. `UserNS=keep-id:uid=65532,gid=65532` bildet deinen Host-Benutzer auf diese UID ab, deshalb gehören die Dateien in `~/medienliste/data` auf dem Host dir. `:Z` setzt das SELinux-Label. Das Root-Dateisystem ist schreibgeschützt (`ReadOnly=true`), geschrieben wird nur in `/data`.

Für Updates gibt es `AutoUpdate=registry` und `systemctl --user enable --now podman-auto-update.timer`.

### Reverse Proxy (OpenResty)

```nginx
server {
    server_name medienliste.example.org;
    # listen/ssl wie gewohnt …
    client_max_body_size 8m;   # Cover-Upload

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_set_header Host              $host;
        proxy_set_header X-Forwarded-For   $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_set_header X-Forwarded-Host  $host;
    }
}
```

Umgang mit den Forwarded-Headern:
- **`X-Forwarded-For`** wird nur ausgewertet, wenn die Verbindung von einer Adresse aus `MEDIENLISTE_TRUSTED_PROXIES` kommt. Die Kette wird von rechts gelesen, damit vom Client gefälschte Einträge ignoriert werden. Die so ermittelte Client-IP erscheint im Log.
- **`Host`, `X-Forwarded-Host` und `X-Forwarded-Proto`** werden für den Aufbau von URLs bewusst *nicht* verwendet. Redirects, Cookie-Attribute und die CSRF-Origin-Prüfung kommen aus `MEDIENLISTE_BASE_URL`. Das verhindert Host-Header-Injection.

### Monitoring

- `GET /healthz`: `200 ok`, solange der Prozess läuft
- `GET /readyz`: `200 ok`, wenn die Datenbank erreichbar ist, sonst `503`
- Im Container: `medienliste healthcheck` prüft `/readyz` (wird von `HEALTHCHECK` bzw. `HealthCmd` verwendet)

## Backup

Die SQLite-Datenbank liegt in `<DATA_DIR>/medienliste.sqlite3` (WAL-Modus). Kopien im laufenden Betrieb werden mit `VACUUM INTO` erstellt und sind immer konsistent:

- **Automatisch:** Alle `MEDIENLISTE_BACKUP_INTERVAL_HOURS` Stunden landet ein Backup in `<DATA_DIR>/backups/medienliste-JJJJMMTT-HHMMSS.sqlite3`. Die letzten `MEDIENLISTE_BACKUP_KEEP` Backups bleiben erhalten.
- **Manuell:**
  ```sh
  podman exec medienliste /usr/local/bin/medienliste backup                       # rotierend nach /data/backups
  podman exec medienliste /usr/local/bin/medienliste backup /data/export.sqlite3  # bestimmte Zieldatei
  ```
- **Per systemd-Timer:** Beispiel in [`deploy/medienliste-backup.service`](deploy/medienliste-backup.service) und [`.timer`](deploy/medienliste-backup.timer)

Für ein vollständiges Backup das ganze Datenverzeichnis sichern (z. B. mit restic oder borg). Dabei die Backup-Dateien statt der Live-Datenbank verwenden. Die Cover liegen in `covers/` und lassen sich notfalls neu laden.

**Wiederherstellen:** Container stoppen, die Backup-Datei als `medienliste.sqlite3` ins Datenverzeichnis kopieren, `medienliste.sqlite3-wal` und `-shm` löschen, dann den Container starten.

## Datenmodell

- `users`: Benutzer (Zuordnung über `sub`), Name, E-Mail, Admin-Flag
- `media`: global geteilte Medien mit Typ `book`, `audiobook` oder `movie` sowie Titel, Untertitel bzw. Originaltitel, Autor(en) bzw. Regie, Sprecher bzw. Darsteller, Beschreibung, ISBN-10/13, ASIN, Erscheinungsjahr, Serie und Band, Sprache, Verlag, lokalem Cover, Quelle und externer ID
- `user_media`: genau ein Status pro Benutzer und Medium (`done` oder `wanted`), dazu das Jahr des Lesens/Hörens/Sehens (`done_year`, änderbar), das Datum der letzten Statusänderung und das Datum des Hinzufügens
- `sessions`, `login_states`: Sitzungen (nur gehashte Tokens) und kurzlebiger OIDC-Zustand

Migrationen liegen versioniert in `migrations/NNNN_name.sql` und werden beim Start automatisch angewendet. Mit `medienliste migrate` lassen sie sich auch einzeln ausführen.

## Berechtigungen und Sicherheit

- Jede Abfrage auf persönliche Daten (`repo::entries`) verlangt die `user_id` aus der Sitzung, nie aus der Anfrage
- Medien sind global, Status und Daten sind privat. Auch Admins sehen keine fremden Listen, nur eine Benutzerübersicht mit der Anzahl der Einträge.
- Bearbeiten eines Mediums ist erlaubt für Admins und für den Ersteller eines manuell angelegten Mediums. Löschen dürfen nur Admins.
- Schutz gegen CSRF: Token pro Sitzung in jedem Formular, Prüfung des `Origin`-Headers, `SameSite=Lax`
- Content-Security-Policy ohne Inline-Skripte, Cookies mit `HttpOnly` und `Secure`
- Beim Hinzufügen aus der Suche werden die Metadaten serverseitig vom Provider geholt und nicht aus der Anfrage übernommen
- Cover werden anhand der Magic Bytes geprüft (JPEG/PNG/WebP/GIF, max. 5 MB)

## Metadaten-Provider

Provider implementieren das Trait `providers::Provider` (`search`, `fetch`) und werden in `Registry::from_config` registriert. Pro Medientyp werden sie der Reihe nach gefragt, der erste mit Treffern gewinnt:

- **Bücher:** Google Books (zuerst deutsche Ausgaben, `langRestrict=de`), danach Open Library (deutsche Werke zuerst)
- **Hörbücher:** Audible-Katalog-API für die Suche. Details holt Audnexus (Sprecher, Serie, Cover), bei Fehlern springt der Audible-Katalog ein.
- **Filme:** TMDB (deutsche Titel und Beschreibungen, Poster). Beim Hinzufügen werden die Details mit Regie, Besetzung und Filmreihe nachgeladen. Nur aktiv, wenn `MEDIENLISTE_TMDB_API_KEY` gesetzt ist.

  > Diese Software nutzt die TMDB-API, ist aber nicht von TMDB unterstützt oder zertifiziert. In der Filmsuche wird ein entsprechender Hinweis angezeigt.

Zusätzlich durchsucht die Suche immer die eigene Bibliothek, also auch manuell angelegte Medien. Treffer aus der eigenen Liste stehen oben und tragen ihren Status als Badge.

## Weitere Sprachen

Alle Texte stehen in `locales/de.ftl`. Für eine neue Sprache die Datei kopieren (z. B. `locales/en.ftl`), übersetzen und in `src/i18n.rs` unter `LOCALES` eintragen. Ein Test stellt sicher, dass jeder in Templates verwendete Schlüssel existiert.

## Entwicklung

```sh
cargo test                     # Unit- und Integrationstests (inkl. Berechtigungen und OIDC-Flow)
cargo clippy --all-targets -- -D warnings
cargo fmt

# lokal starten (Login benötigt einen erreichbaren OIDC-Provider)
MEDIENLISTE_BASE_URL=http://localhost:8080 \
MEDIENLISTE_DATA_DIR=./data \
MEDIENLISTE_OIDC_ISSUER=https://auth.example.org \
MEDIENLISTE_OIDC_CLIENT_ID=medienliste-dev \
cargo run
```

Wichtige Tests:
- `tests/authz.rs`: Benutzer A und B. Geprüft werden Liste, Detail, Suche-Badges, Statistik, Status ändern und entfernen mit fremden Medien-IDs, CSRF/Origin, Bearbeiten und Löschen sowie die Admin-Ansicht.
- `tests/oidc.rs`: kompletter Login gegen einen Fake-IdP mit PKCE, Nonce, Audience, JIT-Provisioning, Aktualisierung der Claims und Admin-Gruppe

Das Container-Image wird von GitHub Actions (`.github/workflows/ci.yml`) für `linux/amd64` und `linux/arm64` gebaut und als `ghcr.io/<github-benutzer>/medienliste` veröffentlicht: `latest` vom `main`-Branch, Versions-Tags bei `v*`-Tags.

Drittsoftware: htmx 2.0.4 (BSD Zero Clause, `static/vendor/htmx.LICENSE`).

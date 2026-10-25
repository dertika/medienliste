# Medienliste – deutsche Texte (Fluent-Format, https://projectfluent.org)
# Weitere Sprachen: Datei kopieren (z. B. en.ftl), übersetzen und in src/i18n.rs eintragen.

app-name = Medienliste
app-description = Bücher, Hörbücher, Filme und Serien im Blick: gelesen, gehört, gesehen oder auf der Wunschliste.

## Navigation
nav-label = Hauptnavigation
nav-search = Suche
nav-list = Meine Liste
nav-new = Neu
nav-stats = Statistik
nav-admin = Verwaltung
nav-logout = Abmelden

## Medientypen
type-book = Buch
type-audiobook = Hörbuch
type-movie = Film
type-series = Serie
type-books = Bücher
type-audiobooks = Hörbücher
type-movies = Filme
type-series-plural = Serien

## Status
status-done = Gelesen/Gehört/Gesehen
status-wanted = Möchte ich lesen/hören/sehen
status-done-book = Gelesen
status-done-audiobook = Gehört
status-done-movie = Gesehen
status-done-series = Gesehen
status-wanted-book = Möchte ich lesen
status-wanted-audiobook = Möchte ich hören
status-wanted-movie = Möchte ich sehen
status-wanted-series = Möchte ich sehen
status-since = seit
status-not-in-list = Nicht in deiner Liste

## Quellen
source-googlebooks = Google Books
source-openlibrary = Open Library
source-audible = Audible
source-tmdb = TMDB
source-dnb = Deutsche Nationalbibliothek
source-manual = Manuell angelegt
source-other = Andere Quelle

## Suche
search-placeholder = Titel, Autor, Regie, ISBN …
search-button = Suchen
search-loading = Suche läuft …
search-intro = Suche nach einem Buch, Hörbuch, Film oder einer Serie – du siehst sofort, ob es schon in deiner Liste ist.
search-no-results = Nichts gefunden.
search-create-manual = Manuell anlegen
search-not-found-create = Nicht das Richtige dabei? Manuell anlegen
search-no-provider = Für diesen Typ ist keine externe Quelle eingerichtet (für Filme und Serien wird ein TMDB-API-Key benötigt). Treffer aus der eigenen Bibliothek werden trotzdem angezeigt, oder du legst den Eintrag manuell an.
search-provider-error = Die externen Quellen sind gerade nicht erreichbar. Treffer aus der eigenen Bibliothek werden trotzdem angezeigt.

## Liste
list-title = Meine Liste
list-empty = Hier ist noch nichts. Such ein Buch, Hörbuch, einen Film oder eine Serie und füge es hinzu.
list-go-search = Zur Suche
list-count = { $count ->
    [one] { $count } Eintrag
   *[other] { $count } Einträge
}
filter-type = Typ
filter-status = Status
filter-sort = Sortierung
filter-all = Alle
filter-text = Filtern
filter-text-placeholder = Titel, Autor, Serie …
filter-apply = Anwenden
sort-changed = Zuletzt geändert
sort-added = Zuletzt hinzugefügt
sort-title = Titel
sort-author = Autor
pager-prev = Zurück
pager-next = Weiter

## Detail & Felder
label-title = Titel
label-subtitle = Untertitel
label-authors = Autor(en)
label-narrators = Sprecher
label-narrated-by = gelesen von
label-cast = Darsteller
label-starring = mit
label-directors = Regie
label-directed-by = Regie:
label-creators = Idee
label-created-by = Idee:
label-network = Sender
label-original-title = Originaltitel
label-movie-series = Filmreihe
label-part = Teil
label-studio = Studio
label-done-year = Jahr
year-unknown = unbekannt
label-series = Serie
label-volume = Band
label-year = Erscheinungsjahr
label-publisher = Verlag
label-language = Sprache
label-source = Quelle
label-description = Beschreibung
label-cover = Cover
label-added = Hinzugefügt am

## Aktionen
action-edit = Bearbeiten
action-delete = Medium löschen
action-remove = Aus meiner Liste entfernen
action-save = Speichern
action-home = Zur Startseite
action-login = Anmelden
action-login-again = Erneut anmelden
confirm-remove = Wirklich aus deiner Liste entfernen?
confirm-delete = Dieses Medium für alle Benutzer löschen? Das kann nicht rückgängig gemacht werden.

## Formular
form-title-new = Medium manuell anlegen
form-title-edit = Medium bearbeiten
form-intro = Nichts gefunden? Hier kannst du ein Buch, Hörbuch, einen Film oder eine Serie selbst erfassen.
form-names-hint = Mehrere durch Komma trennen
form-remove-cover = Cover entfernen
form-error-type = Bitte Buch, Hörbuch, Film oder Serie auswählen.
form-error-done-year = Bitte ein gültiges Jahr angeben (oder leer lassen).
form-error-title = Bitte einen Titel angeben.
form-error-isbn = Die ISBN muss 10 oder 13 Stellen haben.
form-error-asin = Die ASIN muss genau 10 Zeichen (Buchstaben/Ziffern) haben.
form-error-year = Bitte ein gültiges Erscheinungsjahr angeben.
form-error-cover = Das Cover muss ein JPEG-, PNG-, WebP- oder GIF-Bild bis 5 MB sein.

## Statistik
stats-title = Statistik
stats-books-read = Bücher gelesen
stats-audiobooks-heard = Hörbücher gehört
stats-books-wanted = Bücher auf der Wunschliste
stats-audiobooks-wanted = Hörbücher auf der Wunschliste
stats-movies-seen = Filme gesehen
stats-movies-wanted = Filme auf der Wunschliste
stats-series-seen = Serien gesehen
stats-series-wanted = Serien auf der Wunschliste
stats-no-year = ohne Jahr
stats-per-year = Pro Jahr
stats-year = Jahr
stats-empty = Noch nichts gelesen, gehört oder gesehen.
stats-hint = Gezählt wird das Jahr, das beim Status „Gelesen“, „Gehört“ oder „Gesehen“ hinterlegt ist. Es wird beim Setzen des Status vorbelegt und lässt sich auf der Detailseite ändern.

## Verwaltung
admin-title = Verwaltung
admin-intro = Übersicht der Benutzer. Persönliche Listen sind auch für Admins nicht einsehbar.
admin-media-count = { $count ->
    [one] { $count } Medium in der Bibliothek
   *[other] { $count } Medien in der Bibliothek
}
admin-name = Name
admin-email = E-Mail
admin-entries = Einträge
admin-last-login = Letzter Login
admin-role-admin = Admin

## Anmeldung
login-error-title = Anmeldung fehlgeschlagen
login-error-provider = Der Anmeldedienst ist gerade nicht erreichbar. Bitte später erneut versuchen.
login-error-denied = Die Anmeldung wurde abgelehnt oder abgebrochen.
login-error-invalid = Die Anmeldung konnte nicht überprüft werden.
login-error-expired = Die Anmeldung hat zu lange gedauert. Bitte erneut versuchen.
login-error-not-allowed = Dein Konto ist für die Medienliste nicht freigeschaltet. Bitte wende dich an den Administrator.
logged-out-title = Abgemeldet
logged-out-text = Du wurdest von der Medienliste abgemeldet.
logged-out-idp-hint = Beim Anmeldedienst bist du eventuell noch angemeldet. „Anmelden“ meldet dich dann ohne Passwortabfrage wieder an. Um dich ganz abzumelden, melde dich auch beim Anmeldedienst ab.

## Quellen-Hinweise
tmdb-attribution = Film- und Seriendaten sowie Poster von TMDB. Dieses Produkt nutzt die TMDB-API, ist aber nicht von TMDB unterstützt oder zertifiziert.

## Fehler
error-notfound-title = Nicht gefunden
error-notfound-text = Diese Seite oder dieses Medium gibt es nicht.
error-badrequest-title = Ungültige Anfrage
error-badrequest-text = Die Anfrage war unvollständig oder fehlerhaft.
error-forbidden-title = Kein Zugriff
error-forbidden-text = Dafür fehlt dir die Berechtigung.
error-csrf-text = Die Sicherheitsprüfung ist fehlgeschlagen. Bitte die Seite neu laden und es erneut versuchen.
error-internal-title = Fehler
error-internal-text = Da ist etwas schiefgegangen. Bitte später erneut versuchen.

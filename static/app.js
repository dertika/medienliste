// Medienliste – kleine Helfer ohne Inline-Skripte (CSP: script-src 'self').
(function () {
  "use strict";

  // Filterformulare bei Änderung automatisch absenden.
  document.addEventListener("change", function (e) {
    var form = e.target.closest && e.target.closest("form[data-autosubmit]");
    if (form && e.target.tagName === "SELECT") form.requestSubmit ? form.requestSubmit() : form.submit();
  });

  // Bestätigung für Formulare mit data-confirm (ohne htmx).
  document.addEventListener("submit", function (e) {
    var msg = e.target.getAttribute && e.target.getAttribute("data-confirm");
    if (msg && !window.confirm(msg)) e.preventDefault();
  });

  // Im Formular „Manuell anlegen“ nur die zum Typ passenden Felder und Bezeichnungen zeigen
  // (Elemente mit data-types="book audiobook movie").
  function syncTypeForm(form) {
    var checked = form.querySelector("input[name=media_type]:checked");
    if (!checked) return;
    form.querySelectorAll("[data-types]").forEach(function (el) {
      el.hidden = el.getAttribute("data-types").split(" ").indexOf(checked.value) < 0;
    });
    var done = form.querySelector("input[name=status][value=done]");
    var year = form.querySelector(".done-year");
    if (done && year) year.hidden = !done.checked;
  }
  document.querySelectorAll("form[data-type-form]").forEach(function (form) {
    syncTypeForm(form);
    form.addEventListener("change", function () { syncTypeForm(form); });
  });

  // Fehlende Cover (z. B. kein Bild bei Open Library) durch den Platzhalter ersetzen.
  document.addEventListener("error", function (e) {
    var img = e.target;
    if (!img || img.tagName !== "IMG" || !img.hasAttribute("data-fallback")) return;
    var ph = document.createElement("span");
    ph.className = "cover-ph";
    ph.setAttribute("aria-hidden", "true");
    ph.textContent = img.getAttribute("data-fallback");
    img.replaceWith(ph);
  }, true);

  // Service Worker für die PWA (nur statische Dateien werden gecacht).
  var script = document.getElementById("app-js");
  if ("serviceWorker" in navigator && script) {
    window.addEventListener("load", function () {
      navigator.serviceWorker.register(script.dataset.sw, { scope: script.dataset.scope }).catch(function () {});
    });
  }
})();

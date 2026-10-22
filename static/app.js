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

  // Im Formular „Manuell anlegen“ Hörbuch-Felder nur bei Hörbüchern zeigen.
  function syncTypeForm(form) {
    var checked = form.querySelector("input[name=media_type]:checked");
    form.classList.toggle("is-book", !!checked && checked.value === "book");
  }
  document.querySelectorAll("form[data-type-form]").forEach(function (form) {
    syncTypeForm(form);
    form.addEventListener("change", function () { syncTypeForm(form); });
  });

  // Service Worker für die PWA (nur statische Dateien werden gecacht).
  var script = document.getElementById("app-js");
  if ("serviceWorker" in navigator && script) {
    window.addEventListener("load", function () {
      navigator.serviceWorker.register(script.dataset.sw, { scope: script.dataset.scope }).catch(function () {});
    });
  }
})();

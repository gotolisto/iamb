// Theme and window-chrome switches of the mock-up pages. Without JavaScript the pages follow
// the system theme and show macOS chrome.
(function () {
  var root = document.documentElement;
  function read(key) { try { return localStorage.getItem(key); } catch (e) { return null; } }
  function write(key, value) { try { localStorage.setItem(key, value); } catch (e) { /* private window */ } }
  function press(attr, value) {
    document.querySelectorAll("[" + attr + "]").forEach(function (b) {
      b.setAttribute("aria-pressed", String(b.getAttribute(attr) === value));
    });
  }
  function setTheme(theme) {
    if (theme === "light" || theme === "dark") { root.setAttribute("data-theme", theme); } else { theme = "system"; root.removeAttribute("data-theme"); }
    press("data-set-theme", theme);
    write("iamb-mock-theme", theme);
  }
  function setOs(os) {
    if (os !== "win") { os = "mac"; }
    // Frames marked data-os-fixed show one system on purpose (the two are side by side).
    document.querySelectorAll("[data-os]:not([data-os-fixed])").forEach(function (el) { el.setAttribute("data-os", os); });
    press("data-set-os", os);
    write("iamb-mock-os", os);
  }
  document.addEventListener("click", function (event) {
    var b = event.target.closest("[data-set-theme], [data-set-os]");
    if (!b) { return; }
    if (b.hasAttribute("data-set-theme")) { setTheme(b.getAttribute("data-set-theme")); } else { setOs(b.getAttribute("data-set-os")); }
  });
  var query = new URLSearchParams(location.search);
  setTheme(query.get("theme") || read("iamb-mock-theme") || "system");
  setOs(query.get("os") || read("iamb-mock-os") || "mac");
})();

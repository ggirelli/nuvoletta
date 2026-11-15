// Accessibility glue for the app's modal dialogs (all of them carry aria-modal="true").
// Dioxus renders the dialogs; this adds what the markup cannot: Tab stays inside the topmost
// dialog, and focus returns to the control that opened it once it closes. It also sets the
// document language (the interface is English-only).
(function () {
  document.documentElement.lang = document.documentElement.lang || "en";
  var FOCUSABLE =
    'a[href],button:not([disabled]),input:not([disabled]):not([type="hidden"]),select:not([disabled]),' +
    'textarea:not([disabled]),[tabindex]:not([tabindex="-1"])';
  var before = null; // last focused element while no dialog was open
  var opener = null; // the element to give focus back to
  var wasOpen = false;

  function topModal() {
    var all = document.querySelectorAll('[aria-modal="true"]');
    return all.length ? all[all.length - 1] : null;
  }

  document.addEventListener(
    "focusin",
    function (e) {
      if (!topModal()) before = e.target;
    },
    true
  );

  document.addEventListener(
    "keydown",
    function (e) {
      if (e.key !== "Tab") return;
      var m = topModal();
      if (!m) return;
      var items = Array.prototype.filter.call(m.querySelectorAll(FOCUSABLE), function (x) {
        return x.getClientRects().length > 0;
      });
      if (!items.length) {
        e.preventDefault();
        return;
      }
      var first = items[0];
      var last = items[items.length - 1];
      var active = document.activeElement;
      if (!m.contains(active)) {
        e.preventDefault();
        first.focus();
      } else if (e.shiftKey && active === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && active === last) {
        e.preventDefault();
        first.focus();
      }
    },
    true
  );

  new MutationObserver(function () {
    var open = !!topModal();
    if (open && !wasOpen) {
      opener = before;
    } else if (!open && wasOpen) {
      var target = opener;
      opener = null;
      if (target && document.contains(target) && !target.disabled) target.focus();
    }
    wasOpen = open;
  }).observe(document.documentElement, { childList: true, subtree: true });
})();

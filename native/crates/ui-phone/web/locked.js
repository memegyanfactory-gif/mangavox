// The locked page. A phone that is paired can still land here: a link opened from another app
// (the camera's QR scanner, a chat, some home-screen launches) counts as cross-site, so the
// browser leaves out the SameSite=Strict session cookie on that one navigation. A same-origin
// request from this page does carry it: when it shows the phone is signed in, open the app.
(function () {
  'use strict';
  // Opened from a QR link the PC no longer accepts (used or expired).
  if (location.pathname === '/pair') {
    var used = document.getElementById('used');
    if (used) used.hidden = false;
  }
  // Never bounce between here and the app more than once in a short while.
  var KEY = 'mv.locked.retry';
  var now = Date.now();
  try {
    if (now - Number(sessionStorage.getItem(KEY) || 0) < 15000) return;
  } catch (e) { /* storage blocked: the check below still runs once per page load */ }
  if (!window.fetch) return;
  fetch('/api/me', { credentials: 'same-origin', cache: 'no-store' }).then(function (r) {
    if (!r.ok) return;
    try { sessionStorage.setItem(KEY, String(now)); } catch (e) { /* ignore */ }
    location.replace('/');
  }).catch(function () { /* PC unreachable: stay on this page */ });
})();

// Checks that the generator page still has the controls the picture routine uses, by their
// `data-cy` names. Read-only: it opens the model, frame and size menus to look inside them and
// closes them again; it never uploads, types, changes a setting or presses Generate.
// Order matters: a check for a person and the signed-out state are recognised first, and only a
// signed-in page is looked at for its controls (a signed-out page lacks most of them, which says
// nothing about whether the site changed).
// `P` = {generator, names: [data-cy...], model_cy}. Returns {ok: true, signed_in, found: [data-cy...],
// label} or {ok: false, need: "human" | "site", why}.
(async (P) => {
  const q = (cy, root) => (root || document).querySelector('[data-cy="' + cy + '"]');
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const text = (el) => ((el && el.innerText) || "").replace(/\s+/g, " ").trim();
  const fail = (need, why) => ({ ok: false, need, why });
  const until = async (test, ms, step) => {
    const end = Date.now() + ms;
    for (;;) {
      const v = test();
      if (v) return v;
      if (Date.now() > end) return null;
      await sleep(step || 300);
    }
  };
  const escape = async () => {
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await sleep(400);
  };
  // A check that only a person may pass: never touched, handed to the user.
  const human = () =>
    /just a moment|attention required|verify you are human/i.test(document.title || "") ||
    !!document.querySelector("#challenge-form, #challenge-running, #cf-challenge-running") ||
    [...document.querySelectorAll("iframe")].some((f) => {
      const r = f.getBoundingClientRect();
      const shown = f.checkVisibility ? f.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true }) : getComputedStyle(f).visibility !== "hidden";
      return shown && r.width > 60 && r.height > 40 && /recaptcha|hcaptcha|turnstile|challenges\.cloudflare|captcha/i.test(f.src || "");
    });
  const visible = (e) => e.getBoundingClientRect().width > 0;
  // The site's own sign-in page (the generator sends a signed-out visitor there): left alone.
  const signInPage = () => !location.href.startsWith(P.generator) && /(^|[\/.])(log-?in|sign-?in|signin|auth|sso|accounts?)([\/.?#-]|$)/i.test(location.hostname + location.pathname);
  // Signed out, the generator still shows its form, with Log in / Sign in at the top.
  // The site's own sign-in buttons and login modal (named the same in every language), or a
  // visible "Log in" / "Sign in" button. An open login modal is left alone.
  const SIGNED_OUT_CY = ["header-login-button", "header-signup-button", "login-button-modal", "signup-button-modal"];
  const signedOut = (words) => SIGNED_OUT_CY.some((cy) => q(cy)) || [...document.querySelectorAll("a,button")].some((e) => visible(e) && words.test(text(e)));
  const SIGNED_OUT = { ok: true, signed_in: false, found: [], label: "" };

  // 1. A check for a person, 2. the sign-in page: before anything else is judged.
  if (human()) return fail("human", "Magnific is asking to confirm you are a person.");
  if (signInPage()) return SIGNED_OUT;
  if (!location.href.startsWith(P.generator)) {
    location.assign(P.generator);
    return fail("site", "navigated");
  }
  // The page builds itself late (seconds, slower in a hidden window): wait for the form, then
  // for the rest of it (or a sign-in mark) before anything is judged.
  const form = await until(() => q("image-generator-form"), 30000, 500);
  if (form) await until(() => signedOut(/^(log ?in|sign ?in)$/i) || (q("tti-mode-selector-v3-trigger") && q("image-generate-button")), 20000, 500);
  if (human()) return fail("human", "Magnific is asking to confirm you are a person.");
  if (signInPage()) return SIGNED_OUT;
  // 3. Signed out: reported as such; its missing controls are never looked for.
  if (!form) return signedOut(/^(log ?in|sign ?in|sign ?up)$/i) ? SIGNED_OUT : fail("site", "loading");
  if (signedOut(/^(log ?in|sign ?in)$/i)) return SIGNED_OUT;

  // 4. Signed in: which controls are there.
  const seen = new Set();
  const look = () => {
    for (const n of P.names) if (q(n)) seen.add(n);
  };
  look();
  // Most of the proven controls absent: the page has not finished loading (a site that really
  // changed still shows most of them). Reported as loading, never as a changed page.
  if ((P.required || []).filter((n) => seen.has(n)).length * 2 < (P.required || []).length) return fail("site", "loading");

  // Inside the menus: open, look, close.
  for (const [trigger, inside] of [
    ["tti-mode-selector-v3-trigger", "tti-model-selector-popover-model-" + P.model_cy],
    ["image-aspect-ratio-input", "image-generator-aspect-ratio-16:9"],
    ["image-output-axes-trigger", "axis-option-resolution-1k"],
  ]) {
    const t = q(trigger);
    if (!t) continue;
    t.click();
    await until(() => q(inside), 5000);
    look();
    await escape();
    await sleep(300 + Math.floor(Math.random() * 400));
  }
  const go = q("image-generate-button");
  return { ok: true, signed_in: true, found: [...seen], label: text(go).slice(0, 80) };
})(__PARAMS__)

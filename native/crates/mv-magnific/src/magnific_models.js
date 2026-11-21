// Reads the generator's model menu: every model of every provider group (collapsed groups are
// opened, one at a time if the menu only keeps one open), with its exact data-cy id, the words it
// shows, its provider group, and whether its row shows the free marking ("∞ Free gens",
// "Unlimited", "credit-free" or an infinity icon) and no price (a coin icon, or a number that is
// not a size, a time or a chip like "Refs", "New", "40% off"). Sets 1K and unlimited mode first,
// since the menu shows what is free for the current size and mode. It only opens and closes
// menus: nothing is uploaded, typed or generated. Advisory: the Generate guard still decides per
// picture.
// `P` = {generator}. Returns {ok: true, models: [{cy, name, raw, group, free, priced, sizes}]} or
// {ok: false, need: "signin" | "human" | "site", why}.
(async (P) => {
  const q = (cy, root) => (root || document).querySelector('[data-cy="' + cy + '"]');
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const nap = () => sleep(300 + Math.floor(Math.random() * 500));
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
  // The site's own sign-in buttons and login modal (named the same in every language), or a
  // visible "Log in" / "Sign in" button. An open login modal is left alone.
  const SIGNED_OUT_CY = ["header-login-button", "header-signup-button", "login-button-modal", "signup-button-modal"];
  const signedOut = (words) => SIGNED_OUT_CY.some((cy) => q(cy)) || [...document.querySelectorAll("a,button")].some((e) => visible(e) && words.test(text(e)));

  // A check for a person and the sign-in state come first: nothing else is judged before them.
  if (human()) return fail("human", "Magnific is asking to confirm you are a person.");
  if (signInPage()) return fail("signin", "You are not signed in to Magnific.");
  if (!location.href.startsWith(P.generator)) {
    location.assign(P.generator);
    return fail("site", "navigated");
  }
  // The page builds itself late: the form, then the rest of it or a sign-in mark.
  const form = await until(() => q("image-generator-form"), 30000, 500);
  if (form) await until(() => signedOut(/^(log ?in|sign ?in)$/i) || (q("tti-mode-selector-v3-trigger") && q("image-generate-button")), 20000, 500);
  if (human()) return fail("human", "Magnific is asking to confirm you are a person.");
  if (signInPage()) return fail("signin", "You are not signed in to Magnific.");
  if (!form) return signedOut(/^(log ?in|sign ?in|sign ?up)$/i) ? fail("signin", "You are not signed in to Magnific.") : fail("site", "loading");
  if (signedOut(/^(log ?in|sign ?in)$/i)) return fail("signin", "You are not signed in to Magnific.");

  // 1K and unlimited mode on, as the default model uses them.
  const size = q("image-output-axes-trigger");
  if (size && text(size) !== "1K") {
    size.click();
    const opt = await until(() => q("axis-option-resolution-1k"), 5000);
    if (opt) {
      opt.click();
      await sleep(600);
    }
    await escape();
  }
  await nap();
  const unlimited = q("unlimited-mode-toggle-button");
  if (unlimited && text(unlimited) !== "ON") {
    unlimited.click();
    await until(() => text(q("unlimited-mode-toggle-button")) === "ON", 4000);
    await escape();
  }
  await nap();

  const trigger = await until(() => q("tti-mode-selector-v3-trigger"), 20000, 500);
  if (!trigger) return fail("site", "loading");
  trigger.click();
  const PREFIX = "tti-model-selector-popover-model-";
  // The menu's own box: around its rows, or the open popover itself.
  const POPOVER = '[data-radix-popper-content-wrapper], [role="dialog"], [role="menu"], [role="listbox"], [data-cy*="model-selector-popover" i]';
  const rowsNow = () => [...document.querySelectorAll('[data-cy^="' + PREFIX + '"]')].filter((e) => !(e.parentElement && e.parentElement.closest('[data-cy^="' + PREFIX + '"]')));
  const box = () => {
    const r = rowsNow()[0];
    const near = r && r.closest(POPOVER);
    if (near) return near;
    return [...document.querySelectorAll(POPOVER)].find((e) => visible(e) && !e.contains(trigger)) || null;
  };
  if (!(await until(() => rowsNow().length || box(), 6000))) {
    await escape();
    return fail("site", "The model list did not open.");
  }
  // Let the list finish drawing (badges can arrive a moment later).
  await sleep(800);

  // The marks a row can carry.
  const INFINITY = '[data-icon*="infinity" i], [class*="infinity" i], [aria-label*="infinity" i], [aria-label*="unlimited" i], [title*="unlimited" i], [data-cy*="unlimited" i], [data-cy*="infinity" i], [data-cy*="free" i]';
  const COIN = '[data-icon*="coin" i], [class*="coin" i], [aria-label*="coin" i], [aria-label*="credit" i]:not([aria-label*="credit-free" i]):not([aria-label*="credit free" i]), [title*="credit" i]:not([title*="credit-free" i]):not([title*="credit free" i]), [data-cy*="credit" i]:not([data-cy*="credit-free" i]):not([data-cy*="free" i]), [data-cy*="coin" i], [data-cy*="price" i]';
  const SIZE = /\b\d+(?:\.\d+)?\s*K(?:\s*[-–]\s*\d+(?:\.\d+)?\s*K)?\b/gi;
  const read = (e, group) => {
    const raw = text(e);
    const lines = (e.innerText || "").split("\n").map((l) => l.trim()).filter(Boolean);
    const name = lines[0] || raw;
    // What is left once the name and the known chips are gone: a number there is a price.
    const rest = raw
      .replace(name, " ")
      .replace(SIZE, " ")
      .replace(/~?\s*\d+(?:\.\d+)?\s*(?:s|sec|secs|seconds?|min)\b/gi, " ")
      .replace(/\d+\s*%\s*off\b/gi, " ")
      .replace(/\b(?:refs?|new)\b/gi, " ");
    const mark = /∞|free gens?|\bunlimited\b|credit-free/i.test(raw) || !!e.querySelector(INFINITY);
    const priced = /\d/.test(rest) || /\bcredits?\b(?!-free)/i.test(raw) || !!e.querySelector(COIN);
    const sizes = (raw.match(SIZE) || []).map((s) => s.replace(/\s+/g, "").toUpperCase());
    return { cy: (e.getAttribute("data-cy") || "").slice(PREFIX.length), name, raw: raw.slice(0, 200), group, free: mark && !priced, priced, sizes };
  };
  const found = new Map();
  const groupOf = (e) => {
    // The nearest group header above the row: an expandable control in the same section.
    for (let a = e.parentElement, i = 0; a && i < 6; i++, a = a.parentElement) {
      const h = a.querySelector('[aria-expanded]');
      if (h && !h.matches('[data-cy^="' + PREFIX + '"]') && !h.contains(e)) return text(h).split(" ").slice(0, 3).join(" ");
    }
    return "";
  };
  const collect = (group) => {
    for (const e of rowsNow()) {
      const m = read(e, group || groupOf(e));
      if (m.cy && !found.has(m.cy)) found.set(m.cy, m);
    }
  };
  collect("");
  // Every provider group still closed is opened (never a model row, never a buy link), and its
  // rows are read before the next, in case the menu keeps only one open.
  const closed = () => {
    const b = box();
    if (!b) return [];
    return [...b.querySelectorAll('[aria-expanded="false"]')].filter((h) => visible(h) && h !== trigger && !h.closest('[data-cy^="' + PREFIX + '"]') && !h.querySelector('[data-cy^="' + PREFIX + '"]') && !/upgrade|buy|purchase|pricing|subscribe/i.test(text(h)));
  };
  const tried = new Set();
  for (let i = 0; i < 30; i++) {
    const h = closed().find((x) => !tried.has(text(x)));
    if (!h) break;
    const name = text(h);
    tried.add(name);
    h.click();
    await sleep(500);
    collect(name.split(" ").slice(0, 3).join(" "));
  }
  await escape();
  const models = [...found.values()];
  if (!models.length) return fail("site", "The model list did not open.");
  return { ok: true, models };
})(__PARAMS__)

// Makes pictures on Magnific's image generator page, as the signed-in user would by hand: the
// requested model (by its exact data-cy id), 1K or else the model's smallest size, unlimited
// (Relax) mode on when the model has that switch, the prompt rewriter off; drops the reference
// pictures into their slots, types the prompt, presses Generate, waits for the new pictures and
// returns them. It presses Generate only when the model's menu row showed the free marking (or its
// Unlimited switch reads ON) and the button itself shows no price and a free sign ("Unlimited",
// "Relax mode" said to be "Credit-free", "Free", ∞, or the page's "Credit-free" note under it);
// otherwise it stops and says why.
// A check meant for a person is never touched: it is reported so the window can be handed over.
// Controls added to the site after the proven routine (Style and Character slots, the count
// buttons, other models) are used when present and degrade quietly: the Add slot, one picture,
// the default model (said in the answer).
// Every action first checks that this job is still the window's job (the app takes the token
// back when the job is cancelled) and that its time is not up, so a cancelled or late routine
// stops within a moment and never acts after it.
// `P` = {generator, token, budget_ms, reserve_ms, model_name, model_cy, default_name, default_cy,
//        prompt, aspect, count, wait_ms, refs: [{name, b64, slot: "add" | "style" | "character", role}]}.
// Returns {ok: true, images: [{data, w, h, id}], slot_fallback, model, model_fallback, free_label,
//          pressed_by_app (false: the person at the window had already pressed Generate)} or
// {ok: false, need: "signin" | "human" | "paid" | "alert" | "timeout" | "site" | "cancelled", why, detail}.
(async (P) => {
  const fail = (need, why, detail) => ({ ok: false, need, why, detail: detail || "" });
  // This job's token. A token the app took back before the routine began stops it at once.
  const stopped = () => (window.__mangavoxStop || []).includes(P.token);
  if (stopped()) return fail("cancelled", "stopped");
  window.__mangavoxJob = P.token;
  const live = () => window.__mangavoxJob === P.token && !stopped();
  // The routine gives up by itself before the app stops waiting for it.
  const DEADLINE = Date.now() + (P.budget_ms || 600000);
  const STOPPED = { stopped: true };
  const LATE = { late: true };
  const guard = () => {
    if (!live()) throw STOPPED;
    if (Date.now() > DEADLINE) throw LATE;
  };
  const q = (cy, root) => (root || document).querySelector('[data-cy="' + cy + '"]');
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  // A short, uneven pause between actions, the way a person works.
  const nap = async () => {
    await sleep(300 + Math.floor(Math.random() * 500));
    guard();
  };
  const text = (el) => ((el && el.innerText) || "").replace(/\s+/g, " ").trim();
  const until = async (test, ms, step) => {
    const end = Math.min(Date.now() + ms, DEADLINE);
    for (;;) {
      guard();
      const v = test();
      if (v) return v;
      if (Date.now() > end) return null;
      await sleep(step || 300);
    }
  };
  // Every click on the page goes through here: only while this is still the window's job.
  const press = (el) => {
    guard();
    el.click();
  };
  const escape = async () => {
    guard();
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await sleep(400);
  };
  // The first of several possible names a control may have.
  const first = (names, root) => {
    for (const n of names) {
      const e = q(n, root);
      if (e) return e;
    }
    return null;
  };
  // A check that only a person may pass: never touched, handed to the user. A full-page check
  // (the site's own "verify you are human" page) counts as much as one inside the page.
  const human = () =>
    /just a moment|attention required|verify you are human/i.test(document.title || "") ||
    !!document.querySelector("#challenge-form, #challenge-running, #cf-challenge-running") ||
    [...document.querySelectorAll("iframe")].some((f) => {
      const r = f.getBoundingClientRect();
      // The page always carries an invisible reCAPTCHA badge (hidden by CSS): only a check the
      // user can see counts.
      const shown = f.checkVisibility ? f.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true }) : getComputedStyle(f).visibility !== "hidden";
      return shown && r.width > 60 && r.height > 40 && /recaptcha|hcaptcha|turnstile|challenges\.cloudflare|captcha/i.test(f.src || "");
    });
  // Any warning the site shows: the run stops on it.
  const warning = () =>
    [...document.querySelectorAll('[role="alert"], [role="alertdialog"]')]
      .filter((e) => e.getBoundingClientRect().width > 0)
      .map(text)
      .find((t) => t.length > 8 && /error|fail|limit|wrong|try again|not allowed|blocked|violat|unusual|suspend|paused|review|warning|insufficient|not enough|busy|too many/i.test(t));

  const visible = (e) => e.getBoundingClientRect().width > 0;
  // The site's own sign-in page (the generator sends a signed-out visitor there): left alone.
  const signInPage = () => !location.href.startsWith(P.generator) && /(^|[\/.])(log-?in|sign-?in|signin|auth|sso|accounts?)([\/.?#-]|$)/i.test(location.hostname + location.pathname);
  // The site's own sign-in buttons and login modal (named the same in every language), or a
  // visible "Log in" / "Sign in" button. An open login modal is left alone.
  const SIGNED_OUT_CY = ["header-login-button", "header-signup-button", "login-button-modal", "signup-button-modal"];
  const signedOut = (words) => SIGNED_OUT_CY.some((cy) => q(cy)) || [...document.querySelectorAll("a,button")].some((e) => visible(e) && words.test(text(e)));

  // A check for a person and the sign-in state come first: no control is judged before them.
  if (human()) return fail("human", "Magnific is asking to confirm you are a person.");
  if (signInPage()) return fail("signin", "You are not signed in to Magnific.");
  // The pinned window may have been moved to another page by the user: back to the generator,
  // and the caller asks again once it has loaded.
  if (!location.href.startsWith(P.generator)) {
    if (!live()) return fail("cancelled", "stopped");
    location.assign(P.generator);
    return fail("site", "navigated");
  }

  // Slots other than plain references. Their names are not known for sure: the first one found
  // is used, and a reference whose slot is missing (or takes no dropped file) goes to Add.
  const SLOTS = {
    add: ["reference-add-button"],
    style: ["image-style-add-button", "style-reference-add-button", "image-style-reference-add-button", "style-add-button"],
    character: ["image-character-add-button", "character-reference-add-button", "image-character-reference-add-button", "character-add-button"],
  };
  const REMOVE = ["reference-card-remove-button", "style-reference-remove-button", "character-reference-remove-button"];
  const MORE = ["increase-number-images-button", "increment-number-images-button"];
  let COUNT = String(P.count || 1);

  // Tell anyone looking at the window that it is busy (clicks still pass through to the page).
  const banner = document.createElement("div");
  banner.id = "__mangavox_banner";
  banner.textContent = "MangaVox is creating a picture here — please wait";
  banner.style.cssText =
    "position:fixed;top:12px;left:50%;transform:translateX(-50%);z-index:2147483647;pointer-events:none;" +
    "background:rgba(20,20,24,.92);color:#fff;font:600 14px system-ui,sans-serif;padding:10px 18px;border-radius:10px;box-shadow:0 4px 18px rgba(0,0,0,.35)";
  const old = document.getElementById("__mangavox_banner");
  if (old) old.remove();
  document.body.appendChild(banner);

  const run = async () => {
    // The page builds itself late (seconds, slower in a window kept behind): wait for its form,
    // then for the rest of it or a sign-in mark (the self-test's own wait) before anything is
    // judged.
    const form = await until(() => q("image-generator-form"), 30000, 500);
    if (form) await until(() => signedOut(/^(log ?in|sign ?in)$/i) || (q("tti-mode-selector-v3-trigger") && q("image-generate-button")), 20000, 500);
    if (human()) return fail("human", "Magnific is asking to confirm you are a person.");
    if (signInPage()) return fail("signin", "You are not signed in to Magnific.");
    // No form: signed out, or a page that never finished (never read as a changed site).
    if (!form) return signedOut(/^(log ?in|sign ?in|sign ?up)$/i) ? fail("signin", "You are not signed in to Magnific.") : fail("site", "loading");

    // Signed out, the page still shows the form (with Log in / Sign up at the top) but no offer.
    if (signedOut(/^(log ?in|sign ?in)$/i)) return fail("signin", "You are not signed in to Magnific.");
    const early = warning();
    if (early) return fail("alert", early.slice(0, 200));

    // The model asked for, found by its exact data-cy id (no prefix assumed); when the menu no
    // longer lists it, the default model (the one the proven routine always used), and the answer
    // says so. The menu is always opened, even when the model is already selected, because its
    // row's free marking is read and remembered: no marking, no press.
    let MODEL = P.model_name;
    let modelFallback = false;
    const PREFIX = "tti-model-selector-popover-model-";
    const INFINITY = '[data-icon*="infinity" i], [class*="infinity" i], [aria-label*="infinity" i], [aria-label*="unlimited" i], [title*="unlimited" i], [data-cy*="unlimited" i], [data-cy*="infinity" i], [data-cy*="free" i]';
    // A price marker. "Credit-free" labels (the Relax note's own tooltip) are not prices.
    const COIN = '[data-icon*="coin" i], [class*="coin" i], [aria-label*="coin" i], [aria-label*="credit" i]:not([aria-label*="credit-free" i]):not([aria-label*="credit free" i]), [title*="credit" i]:not([title*="credit-free" i]):not([title*="credit free" i]), [data-cy*="credit" i]:not([data-cy*="credit-free" i]):not([data-cy*="free" i]), [data-cy*="coin" i], [data-cy*="price" i]';
    // A row is free when it shows the free marking and no price (a coin icon, or a number that is
    // not the name, a size, a time or a chip like "Refs", "New", "40% off").
    const rowFree = (e) => {
      const raw = text(e);
      const name = ((e.innerText || "").split("\n").map((l) => l.trim()).filter(Boolean)[0]) || "";
      const rest = raw
        .replace(name, " ")
        .replace(/\b\d+(?:\.\d+)?\s*K(?:\s*[-–]\s*\d+(?:\.\d+)?\s*K)?\b/gi, " ")
        .replace(/~?\s*\d+(?:\.\d+)?\s*(?:s|sec|secs|seconds?|min)\b/gi, " ")
        .replace(/\d+\s*%\s*off\b/gi, " ")
        .replace(/\b(?:refs?|new)\b/gi, " ");
      const mark = /∞|free gens?|\bunlimited\b|credit-free/i.test(raw) || !!e.querySelector(INFINITY);
      const priced = /\d/.test(rest) || /\bcredits?\b(?!-free)/i.test(raw) || !!e.querySelector(COIN);
      return mark && !priced;
    };
    let ROW_FREE = false;
    const trigger = q("tti-mode-selector-v3-trigger");
    if (!trigger) return fail("site", "loading");
    press(trigger);
    // A model inside a closed provider group: the groups are opened one at a time until its row
    // shows (never a model row, never a buy link).
    const POPOVER = '[data-radix-popper-content-wrapper], [role="dialog"], [role="menu"], [role="listbox"], [data-cy*="model-selector-popover" i]';
    const findRow = async (cy) => {
      const row = () => q(PREFIX + cy);
      let opt = await until(row, 4000);
      const tried = new Set();
      for (let i = 0; !opt && i < 30; i++) {
        const any = document.querySelector('[data-cy^="' + PREFIX + '"]');
        const b = (any && any.closest(POPOVER)) || [...document.querySelectorAll(POPOVER)].find((e) => visible(e) && !e.contains(trigger));
        if (!b) break;
        const h = [...b.querySelectorAll('[aria-expanded="false"]')].find((x) => visible(x) && x !== trigger && !tried.has(text(x)) && !x.closest('[data-cy^="' + PREFIX + '"]') && !x.querySelector('[data-cy^="' + PREFIX + '"]') && !/upgrade|buy|purchase|pricing|subscribe/i.test(text(x)));
        if (!h) break;
        tried.add(text(h));
        press(h);
        opt = await until(row, 1500);
      }
      return opt;
    };
    let opt = await findRow(P.model_cy);
    if (!opt && P.default_cy && P.default_cy !== P.model_cy) {
      MODEL = P.default_name;
      modelFallback = true;
      opt = await findRow(P.default_cy);
    }
    if (opt) {
      await sleep(600);
      ROW_FREE = rowFree(opt);
      if (text(q("tti-mode-selector-v3-trigger")) !== MODEL) {
        await nap();
        press(opt);
        await until(() => text(q("tti-mode-selector-v3-trigger")) === MODEL, 5000);
      }
    }
    await escape();
    if (!opt) return fail("site", "The model could not be found in Magnific's model list.", MODEL);
    if (text(q("tti-mode-selector-v3-trigger")) !== MODEL) return fail("site", "The model could not be picked in Magnific's model list.", MODEL);
    const MODEL_CY = modelFallback ? P.default_cy : P.model_cy;
    await nap();

    // How many pictures one press makes; a page that cannot be set to that many makes one.
    const setCount = async () => {
      for (let i = 0; i < 8 && text(q("number-images-value")) !== COUNT; i++) {
        const now = parseInt(text(q("number-images-value")), 10);
        const button = now > Number(COUNT) ? q("decrease-number-images-button") : first(MORE);
        if (!button || button.disabled || isNaN(now)) break;
        press(button);
        await sleep(250);
      }
    };
    await setCount();
    if (text(q("number-images-value")) !== COUNT && COUNT !== "1") {
      COUNT = "1";
      await setCount();
    }
    await nap();

    // The frame.
    const ratio = q("image-aspect-ratio-input");
    if (ratio && P.aspect && text(ratio) !== P.aspect) {
      press(ratio);
      const opt = await until(() => q("image-generator-aspect-ratio-" + P.aspect), 5000);
      if (opt) {
        press(opt);
        await sleep(500);
      }
      await escape();
    }
    await nap();

    // The size: 1K when the model offers it (the only size the unlimited offer covers), else the
    // smallest size it offers. The size actually set is what the free guard compares with. A model
    // without a size menu does without (SIZE stays "", and the menu must still be absent then).
    const sizeBox = q("image-output-axes-trigger");
    let SIZE = sizeBox ? "1K" : "";
    if (sizeBox && text(sizeBox) !== "1K") {
      press(sizeBox);
      const SIZE_PREFIX = "axis-option-resolution-";
      const opts = await until(() => {
        const all = [...document.querySelectorAll('[data-cy^="' + SIZE_PREFIX + '"]')]
          .map((e) => ({ e, k: (e.getAttribute("data-cy") || "").slice(SIZE_PREFIX.length).toLowerCase() }))
          .filter((o) => /^\d+(?:[.-]\d+)?k$/.test(o.k))
          .map((o) => ({ e: o.e, n: parseFloat(o.k.replace("-", ".")), label: o.k.replace("-", ".").toUpperCase() }));
        return all.length ? all : null;
      }, 5000);
      const pick = opts ? opts.find((o) => o.n === 1) || opts.sort((a, b) => a.n - b.n)[0] : null;
      if (!pick) {
        await escape();
        return fail("site", "The picture size could not be set.");
      }
      SIZE = pick.label;
      if (text(q("image-output-axes-trigger")) !== SIZE) {
        press(pick.e);
        await sleep(600);
      }
      await escape();
      if (text(q("image-output-axes-trigger")) !== SIZE) return fail("site", "The picture size could not be set.", SIZE);
    }
    await nap();

    // Unlimited mode on when the model has the switch (a model without it does without), the
    // prompt rewriter off (the prompt is used as written).
    const TOGGLE = !!q("unlimited-mode-toggle-button");
    const unlimited = q("unlimited-mode-toggle-button");
    if (unlimited && text(unlimited) !== "ON") {
      press(unlimited);
      await until(() => text(q("unlimited-mode-toggle-button")) === "ON", 4000);
      await escape();
    }
    // The switch, when the model has one, reads ON; a model that had none still has none.
    const toggleOk = () => (TOGGLE ? text(q("unlimited-mode-toggle-button")) === "ON" : !q("unlimited-mode-toggle-button"));
    // The row's marking follows the size and the switch (Nano Banana's row shows its price until
    // Unlimited is on): read once more now that they are set, and remembered.
    if (!ROW_FREE) {
      await nap();
      const t2 = q("tti-mode-selector-v3-trigger");
      if (!t2) return fail("site", "loading");
      press(t2);
      const again = await findRow(MODEL_CY);
      if (again) {
        await sleep(600);
        ROW_FREE = rowFree(again);
      }
      await escape();
    }
    // The model's own free offer: its row showed the free marking, or its Unlimited switch reads
    // ON (the offer the proven routine relied on). Neither: nothing more is done.
    const rowOk = () => ROW_FREE || (TOGGLE && toggleOk());
    if (!rowOk() || text(q("tti-mode-selector-v3-trigger")) !== MODEL) return fail("paid", "Magnific does not show this picture as free.", "model row not marked free: " + text(opt).slice(0, 80));
    const smart = q("smart-prompt-toggle");
    if (smart && smart.getAttribute("aria-checked") === "true") {
      press(smart);
      await sleep(300);
    }
    await nap();

    // References: out with whatever is there, in with this picture's own, in order (the prompt's
    // "Image N" is the N-th uploaded).
    const box = q("image-references-input");
    const count = () => parseInt((text(box).match(/(\d+)\s*\/\s*\d+/) || [0, "0"])[1], 10);
    const cards = () => REMOVE.reduce((n, cy) => n + form.querySelectorAll('[data-cy="' + cy + '"]').length, 0);
    const pics = () => form.querySelectorAll("img").length;
    for (let i = 0; i < 30; i++) {
      const rm = first(REMOVE, form);
      if (!rm) break;
      press(rm);
      await sleep(500);
    }
    let slotFallback = false;
    const drop = async (target, r) => {
      const bytes = Uint8Array.from(atob(r.b64), (c) => c.charCodeAt(0));
      const dt = new DataTransfer();
      dt.items.add(new File([bytes], r.name, { type: "image/jpeg" }));
      const before = [count(), cards(), pics()];
      for (const type of ["dragenter", "dragover", "drop"]) {
        guard();
        target.dispatchEvent(new DragEvent(type, { bubbles: true, cancelable: true, dataTransfer: dt }));
        await sleep(150);
      }
      return before;
    };
    const landed = (before) => count() > before[0] || cards() > before[1] || pics() > before[2];
    const refs = (P.refs || []).slice(0, 14);
    for (let n = 0; n < refs.length; n++) {
      const r = refs[n];
      let done = false;
      if (r.slot && r.slot !== "add") {
        const target = first(SLOTS[r.slot] || []);
        if (target) {
          const before = await drop(target, r);
          done = !!(await until(() => landed(before), 15000, 500));
          if (!done) await escape();
        }
        if (!done) slotFallback = true;
      }
      if (!done) {
        const target = q("reference-add-button");
        if (!target) return fail("site", "The place to add a reference picture was not found.");
        const before = await drop(target, r);
        // Named by its place and role ("Image 3, face and body"), never by a file name.
        if (!(await until(() => landed(before), 45000, 500))) return fail("site", "Reference picture " + (n + 1) + " (" + (r.role || "reference") + ") did not upload.");
      }
      if (human()) return fail("human", "Magnific is asking to confirm you are a person.");
      await nap();
    }

    // The prompt.
    const ed = (q("image-prompt-input") || document).querySelector('[role="textbox"]');
    if (!ed) return fail("site", "The prompt box was not found.");
    guard();
    ed.focus();
    window.getSelection().selectAllChildren(ed);
    // One line: a line break typed into this box acts as Enter, and Enter starts a picture by itself
    // (seen live: the picture began before the routine's own checks and press). Never type one.
    document.execCommand("insertText", false, String(P.prompt).replace(/\s*[\r\n]+\s*/g, " ").trim());
    await sleep(900);

    // Pressed only when the page says this picture costs nothing.
    // The button comes alive once the prompt is in and the references have finished uploading;
    // its own label then says what the picture costs ("Generate Unlimited", or a price).
    const go = await until(() => {
      const b = q("image-generate-button");
      return b && !b.disabled ? b : null;
    }, 90000, 500);
    const label = text(go || q("image-generate-button"));
    const state = "model " + text(q("tti-mode-selector-v3-trigger")) + ", size " + text(q("image-output-axes-trigger")) + ", images " + text(q("number-images-value")) + ", unlimited mode " + text(q("unlimited-mode-toggle-button")) + ", button: " + label.slice(0, 60);
    if (!go) return fail("site", "The Generate button did not become ready.", state);
    // "Generate Unlimited", or, once the fast allowance is used, "Generate Relax mode", which the
    // page itself calls "Credit-free generations" (slower, still free). Only the button, its own
    // tooltip and the unlimited toggle's own box are read, never the rest of the page.
    const tip = (go.getAttribute("aria-describedby") || "").split(/\s+/).filter(Boolean).map((id) => text(document.getElementById(id))).join(" ");
    let toggleBox = "";
    for (let a = q("unlimited-mode-toggle-button"), i = 0; a && i < 3; i++, a = a.parentElement) {
      const t = text(a);
      if (t.length > 200) break;
      toggleBox = t;
    }
    // Free in the button's own words: the label starts with "Generate", shows no price (no number
    // but the size and the count, no "credits", no coin icon, nothing to buy or upgrade), and
    // carries a free sign: "Unlimited", "Relax mode" said to be credit-free, "Free", or an
    // infinity sign (∞ in the label, or an infinity icon inside the button itself).
    // The page states Relax mode is credit-free in a short note of its own (the first, proven routine
    // read it there for months): a short element that says both "Relax mode" and "credit-free".
    const relaxNote = () => [...document.querySelectorAll("body *")].some((e) => { const t = text(e); return t.length < 120 && /\brelax mode\b/i.test(t) && /credit-free/i.test(t) && !/\d+\s*credits?\b/i.test(t); });
    const sizeWord = SIZE ? new RegExp("\\b" + SIZE.replace(".", "\\.") + "\\b", "gi") : /(?!)/;
    // The short note the page writes right under the button for a free model ("Credit-free
    // generations", seen live with Seedream 4 and a plain "Generate" label): read only within the
    // button's own few parents, and only while it shows no credit count.
    const buttonNote = (b) => {
      for (let a = b && b.parentElement, i = 0; a && i < 3; i++, a = a.parentElement) {
        const t = text(a);
        if (t.length > 200) break;
        if (/credit-free/i.test(t)) return !/\d+\s*credits?\b/i.test(t) && !a.querySelector(COIN);
      }
      return false;
    };
    const freeLabel = (l, b) => {
      const rest = l.replace(sizeWord, " ").replace(new RegExp("\\b" + COUNT + "\\b", "g"), " ");
      // "Relax mode" is Magnific's own name for its credit-free queue (seen live: the button reads
      // "Generate Relax mode" with "Credit-free generations" under it, and the owner confirmed it
      // costs nothing). It counts as free only with no price anywhere on the button (checked above).
      const relaxFree = /\brelax mode\b/i.test(l) || ([l, tip, toggleBox].some((t) => /credit-free/i.test(t)) || relaxNote());
      // A price always shows a number (caught by the digit test). A coin or credit icon on a button
      // whose own words say it is free (Unlimited, Relax mode, Free) is the plan's credit icon, not a
      // price: seen live on the "Generate Relax mode" button, which costs nothing.
      const saysFree = /\b(unlimited|relax mode|free)\b/i.test(l);
      const costs = !b || /\d/.test(rest) || /\bcredits?\b(?!-free)/i.test(l) || /upgrade|buy|purchase|subscribe|top ?up/i.test(l) || (!saysFree && !!b.querySelector(COIN));
      if (costs) return false;
      const freeSign = /\bunlimited\b/i.test(l) || relaxFree || /\bfree\b/i.test(l) || /∞/.test(l) || !!b.querySelector(INFINITY) || (ROW_FREE && buttonNote(b));
      return /^generate\b/i.test(l) && freeSign;
    };
    // Still signed in, right before the guard.
    if (signedOut(/^(log ?in|sign ?in)$/i)) return fail("signin", "You are not signed in to Magnific.");
    if (!rowOk() || text(q("tti-mode-selector-v3-trigger")) !== MODEL || text(q("image-output-axes-trigger")) !== SIZE || text(q("number-images-value")) !== COUNT || !toggleOk() || !freeLabel(label, go)) {
      // For the log only: which condition said no, and the button as the page drew it.
      const why = [
        rowOk() ? "" : "row not free",
        text(q("tti-mode-selector-v3-trigger")) !== MODEL ? "model differs" : "",
        text(q("image-output-axes-trigger")) !== SIZE ? "size differs" : "",
        text(q("number-images-value")) !== COUNT ? "count " + text(q("number-images-value")) + " not " + COUNT : "",
        toggleOk() ? "" : "unlimited switch",
        freeLabel(label, go) ? "" : "label/button" + (go && go.querySelector(COIN) ? " (price marker inside)" : ""),
      ].filter(Boolean).join(", ");
      // The price markers themselves (tag and attributes only), so the log shows what matched.
      const markers = go ? [...go.querySelectorAll(COIN)].map((e) => "<" + e.tagName.toLowerCase() + [...e.attributes].map((a) => " " + a.name + '="' + a.value.slice(0, 60) + '"').join("") + ">").join(" ").slice(0, 600) : "";
      return fail("paid", "Magnific does not show this picture as free.", state + " | failed: " + why + " | markers: " + markers + " | button text: " + text(go).slice(0, 120));
    }

    // The pictures in the results feed by Magnific id, read from attributes only (src, srcset,
    // data-src, a video's poster), never from rendered sizes: the window may sit behind others,
    // where the page draws nothing new on screen. Id -> the picture's address.
    const PID = /\/production\/(\d+)\//;
    const feedPics = () => {
      const m = new Map();
      for (const e of document.querySelectorAll("img, source, video")) {
        for (const a of ["src", "srcset", "data-src", "poster"]) {
          const v = e.getAttribute(a) || "";
          const hit = v.match(PID);
          if (!hit) continue;
          // srcset: the first address in the list.
          const url = a === "srcset" ? v.trim().split(/\s+/)[0] : e.tagName === "IMG" && a === "src" ? e.src : v;
          if (!m.has(hit[1]) || (e.tagName === "IMG" && a === "src")) m.set(hit[1], { url, el: e });
          break;
        }
      }
      return m;
    };
    const ids = () => [...feedPics().keys()];
    // The feed scrolled back to its newest pictures (the person at the window may have scrolled
    // it down; a scrolled feed may not keep its newest cards in the page). Scrolling only.
    const feedTop = () => {
      const any = [...feedPics().values()][0];
      for (let a = any && any.el.parentElement; a && a !== document.body; a = a.parentElement) {
        const s = getComputedStyle(a);
        if (/(auto|scroll)/.test(s.overflowY) && a.scrollHeight > a.clientHeight + 4) {
          if (a.scrollTop > 0) a.scrollTop = 0;
          return;
        }
      }
      if (window.scrollY > 0) window.scrollTo(0, 0);
    };
    // Picture ids only grow. The last run's picture can show up on the page after this run began
    // (it was then taken for this one, one picture behind): only an id above every one seen before,
    // and above the last one this routine returned, is this run's.
    const norm = (x) => x.toLowerCase().replace(/[^a-z0-9]+/g, " ").trim();
    const whole = norm(P.prompt);
    const lead = whole.slice(0, 40).trim();
    const STOP = new Set("the and with from for into onto over that this his her its their them they was are has have who what one two all out off but not".split(" "));
    const content = (ws) => ws.filter((w) => w.length >= 3 && !STOP.has(w) && !/^\d+$/.test(w));
    const promptWords = new Set(content(whole.split(" ")));
    // The text of a picture's own card: a few levels up, never the list around many cards.
    const cardText = (pid) => {
      const hit = feedPics().get(String(pid));
      for (let a = hit && hit.el, i = 0; a && i < 8; i++, a = a.parentElement) {
        if (a.querySelectorAll('img[src*="/production/"]').length > 4) return "";
        const t = text(a);
        if (t.length >= 8) return t;
      }
      return "";
    };
    // A card shows a snippet of its picture's prompt ("View: High wide view from the right
    // side"), or the prompt itself. The same account makes pictures elsewhere at the same time
    // (seen live: someone else's classroom photo was taken as a frame), so a new picture is
    // taken only when its card shows THIS prompt ("ours"), or, with no prompt on the card at all
    // ("blank"), under the strict rules of `blankWhy` below. A card that shows any other prompt
    // ("foreign") is never taken, however long the wait.
    // Words a card shows besides a prompt: sizes, counts, the card's own buttons and states.
    const UI_WORDS = /^(\d+(\.\d+)?k|\d+|status|download|share|copy|delete|more|edit|upscale|variations?|reuse|retry|remix|video|image|images|animate|favorite|like|open|new|by|you|ago|s|m|h|min|mins|sec|secs|just|now|unlimited|relax|mode|free|generating|queued|queue|in|processing|done|failed)$/;
    const OTHER_MODEL = /\b(nano banana|seedream|flux|imagen|mystic|ideogram|gpt|kling|recraft|hidream|qwen|midjourney|runway|reve|minimax|hunyuan|seedance|veo|gemini|dall e)\b/g;
    const verdict = (pid) => {
      const raw = cardText(pid);
      const t = norm(raw);
      const modelWords = new Set(norm(MODEL).split(" "));
      const words = t.split(" ").filter((w) => w && !UI_WORDS.test(w) && !modelWords.has(w));
      const rest = words.join(" ");
      const cw = content(words);
      if (!cw.length) {
        // No prompt on the card. What it does show must be what was asked for.
        const otherModel = (t.match(OTHER_MODEL) || []).find((m) => !norm(MODEL).includes(m));
        if (otherModel) return { kind: "foreign", raw, why: "card shows another model (" + otherModel + ")" };
        const size = (raw.match(/\b\d+(?:\.\d+)?\s?K\b/gi) || []).map((s) => s.replace(/\s/g, "").toUpperCase()).find((s) => SIZE && s !== SIZE.toUpperCase());
        if (size) return { kind: "foreign", raw, why: "card shows another size (" + size + ")" };
        const frame = (raw.match(/\b\d{1,2}:\d{1,2}\b/g) || []).find((r) => P.aspect && r !== P.aspect);
        if (frame) return { kind: "foreign", raw, why: "card shows another frame (" + frame + ")" };
        return { kind: "blank", raw, why: "no prompt on the card" };
      }
      // Ours: the prompt's opening (first 40 normalised characters) on the card, the card's
      // snippet found whole in this prompt, or most of the card's words in this prompt.
      const hits = cw.filter((w) => promptWords.has(w)).length;
      if (lead.length >= 12 && t.includes(lead)) return { kind: "ours", raw, why: "card shows the prompt's opening" };
      if (rest.length >= 16 && whole.includes(rest)) return { kind: "ours", raw, why: "card shows a part of the prompt" };
      if (cw.length >= 3 && hits / cw.length >= 0.75) return { kind: "ours", raw, why: "card words in the prompt " + hits + "/" + cw.length };
      return { kind: "foreign", raw, why: "card shows another prompt (" + hits + "/" + cw.length + " words in ours)" };
    };
    // What shows the page is working on a picture: a spinner, a progress bar, a busy or loading
    // element, a queue entry, or a short "Generating" / "In queue" / "40%" text. Read only, from
    // attributes and short texts (the prompt box is never read). Anything unsure counts as
    // working, so the routine then simply waits its full time as before.
    const BUSY_SEL = '[role="progressbar"], [aria-busy="true"], [class*="spinner" i], [class*="spin" i], [class*="loader" i], [class*="loading" i], [class*="skeleton" i], [class*="progress" i], [class*="pending" i], [class*="queue" i], [data-cy*="loading" i], [data-cy*="progress" i], [data-cy*="queue" i], [data-cy*="pending" i], [data-cy*="generating" i]';
    const busyMarks = () => [...document.querySelectorAll(BUSY_SEL)].filter(visible);
    const BUSY_WORDS = /\b(generating|in queue|queued|waiting in (the )?queue|processing|in progress|starting)\b|\b\d{1,3}\s?%/i;
    // Read within the results feed only (the scrolling list around its pictures), never the
    // page's free-offer texts; with no picture in the feed yet, within the page's main area.
    const feedRoot = () => {
      const any = [...feedPics().values()][0];
      for (let a = any && any.el.parentElement; a && a !== document.body; a = a.parentElement) {
        const s = getComputedStyle(a);
        if (/(auto|scroll)/.test(s.overflowY) && a.scrollHeight > a.clientHeight + 4) return a;
      }
      return document.querySelector("main") || document.body;
    };
    const busyWords = () =>
      [...feedRoot().querySelectorAll("*")].some((e) => {
        if (e.childElementCount || e.closest('textarea, input, [contenteditable=""], [contenteditable="true"]')) return false;
        const t = text(e);
        return t.length > 0 && t.length < 80 && BUSY_WORDS.test(t);
      });
    // The page's state for the log: the button, the busy marks, any toast or status line.
    const pageState = (busyBefore) => {
      const b = q("image-generate-button");
      const marks = busyMarks();
      const toasts = [...document.querySelectorAll('[role="status"], [role="alert"], [class*="toast" i], [class*="snackbar" i]')].filter(visible).map(text).filter(Boolean).join(" / ").slice(0, 300);
      return "button: \"" + text(b).slice(0, 60) + "\"" + (b && b.disabled ? " (disabled)" : "") + (b && b.getAttribute("aria-busy") === "true" ? " (busy)" : "") + ", busy marks " + marks.length + " (new " + marks.filter((m) => !(busyBefore && busyBefore.has(m))).length + "), busy words " + (busyWords() ? "yes" : "no") + ", toasts: " + (toasts || "none");
    };
    feedTop();
    await sleep(600);
    guard();
    const before = new Set(ids());
    const floor = Math.max(Number(window.__mangavoxLast || 0), ...[...before].map(Number), 0);
    await nap();
    // Right before the press, with nothing awaited from here on: still this job, no check for a
    // person, still signed in, no warning, and the button, read again, is the same one and still
    // says free with the same settings.
    guard();
    if (human()) return fail("human", "Magnific is asking to confirm you are a person.");
    if (signedOut(/^(log ?in|sign ?in)$/i)) return fail("signin", "You are not signed in to Magnific.");
    const late = warning();
    if (late) return fail("alert", late.slice(0, 200));
    const button = q("image-generate-button");
    const again = text(button);
    // The "Relax mode" words on the button come and go (seen live: "Generate Relax mode", then plain
    // "Generate" a moment later, same button, still credit-free). So the label may lose or gain them,
    // but nothing else: and the button must still show no price (no number, no "credits", no buy
    // words), checked on what it shows now.
    const core = (s) => s.replace(/\brelax mode\b/gi, " ").replace(/\s+/g, " ").trim().toLowerCase();
    const shownPrice = /\d/.test(again.replace(sizeWord, " ").replace(new RegExp("\\b" + COUNT + "\\b", "g"), " ")) || /\bcredits?\b(?!-free)/i.test(again) || /upgrade|buy|purchase|subscribe|top ?up/i.test(again);
    const sameOffer = again === label || (core(again) === core(label) && !shownPrice && freeLabel(label, button));
    // Pressed already by the person at the window (seen live: the owner pressed this same free
    // button while the routine was checking it): nothing is pressed again; this run waits for
    // that picture as its own.
    const pressedAlready = button === go && go.disabled && core(again) === core(label) && !shownPrice;
    if (button !== go || (go.disabled && !pressedAlready) || !sameOffer || !rowOk() || !(freeLabel(again, button) || sameOffer) || !toggleOk() || text(q("image-output-axes-trigger")) !== SIZE || text(q("tti-mode-selector-v3-trigger")) !== MODEL || text(q("number-images-value")) !== COUNT) {
      return fail("paid", "Magnific does not show this picture as free.", "changed before the press: first \"" + label.slice(0, 60) + "\", now \"" + again.slice(0, 60) + "\"" + (button !== go ? ", other button" : "") + (go.disabled ? ", disabled" : "") + (shownPrice ? ", price shown" : "") + (rowOk() ? "" : ", row") + (toggleOk() ? "" : ", switch"));
    }
    // The busy marks already there before the press (a mark that was always on the page says
    // nothing about this job).
    const busyBefore = new Set(busyMarks());
    if (!pressedAlready) go.click();

    // One job at a time: wait for its pictures before anything else is asked for, leaving time
    // before the deadline to bring them back.
    const want = Number(COUNT);
    const end = Math.min(Date.now() + (P.wait_ms || 300000), DEADLINE - (P.reserve_ms || 90000));
    // New pictures: an id above every one in the feed before the press (and above the last one
    // returned). The same account making pictures elsewhere (another window, the owner by hand)
    // adds new pictures too: only those whose card shows this prompt are taken, a card with no
    // prompt only by `blankWhy`, and one that ever showed another prompt never.
    const EXPECT_MS = P.expect_ms || 240000;
    const appeared = new Map();
    const foreign = new Map();
    let lastFresh = { all: [], matched: [], v: new Map() };
    const fresh = () => {
      const all = [...new Set(ids().filter((x) => !before.has(x)).map(Number).filter((x) => x > floor))].sort((a, b) => b - a);
      const v = new Map();
      for (const id of all) {
        if (!appeared.has(id)) appeared.set(id, Date.now());
        const c = verdict(id);
        if (c.kind === "foreign" && !foreign.has(id)) foreign.set(id, c);
        v.set(id, foreign.get(id) || c);
      }
      const matched = all.filter((x) => v.get(x).kind === "ours");
      lastFresh = { all, matched, v };
      return lastFresh;
    };
    // A card with no prompt on it is taken only as the one new picture since the press (none
    // other ever appeared), in the expected time after the press, and standing 20 s. "" = take.
    const blankWhy = (f, id) => {
      const c = f.v.get(id) || foreign.get(id);
      if (!c) return "left the feed";
      if (c.kind !== "blank") return c.why;
      if (appeared.size !== 1) return "no prompt on the card and " + appeared.size + " new pictures since the press";
      const after = appeared.get(id) - pressedAt;
      if (after > EXPECT_MS) return "no prompt on the card and it appeared " + Math.round(after / 1000) + " s after the press";
      if (Date.now() - appeared.get(id) < 20000) return "no prompt on the card, not yet standing 20 s";
      return "";
    };
    // Why each new picture was taken or skipped, for the log.
    const candidates = (taken) =>
      [...appeared.keys()]
        .map((id) => {
          const c = lastFresh.v.get(id) || foreign.get(id) || { why: "left the feed", raw: "" };
          const ok = taken.includes(id);
          return id + (ok ? " taken: " + c.why : " skipped: " + (c.kind === "blank" ? blankWhy(lastFresh, id) || "no prompt on the card" : c.why)) + ' "' + String(c.raw || "").slice(0, 60) + '"';
        })
        .join("; ");
    // What the feed showed while waiting, every ~20 s: the newest 3 ids with their card text.
    const seen = [];
    const look = () => {
      const top = ids().map(Number).sort((a, b) => b - a).slice(0, 3);
      seen.push(Math.round((Date.now() - pressedAt) / 1000) + "s: " + (top.map((x) => x + " \"" + cardText(x).slice(0, 60) + "\"").join(", ") || "no pictures in the feed"));
      window.__mangavoxSeen = seen.slice(-30);
    };
    const pressedAt = Date.now();
    let lastLook = pressedAt;
    let got = [];
    let how = "";
    let firstSeen = 0;
    // RUN1-18: a job that made no picture at all waited its full 5 minutes in silence. After
    // ~2 minutes without any new picture, the page must show the job (a spinner card, a queue
    // entry, a busy button); when two looks 10 s apart find none, the site never took it: the
    // routine ends with a plain message. Nothing is pressed again; the user decides.
    const START_CHECK_MS = P.start_check_ms || 120000;
    let startShown = false;
    let startMisses = 0;
    let lastStartLook = 0;
    const jobShown = () => {
      const b = q("image-generate-button");
      if (b && (b.disabled || b.getAttribute("aria-busy") === "true" || /\b(generating|queued|in queue|waiting)\b/i.test(text(b)))) return true;
      if (busyMarks().some((m) => !busyBefore.has(m))) return true;
      return busyWords();
    };
    while (Date.now() < end) {
      await sleep(2000);
      guard();
      if (human()) return fail("human", "Magnific is asking to confirm you are a person.");
      feedTop();
      const now = fresh();
      if (now.all.length && !firstSeen) firstSeen = Date.now();
      if (!firstSeen && !startShown && Date.now() - pressedAt >= START_CHECK_MS && Date.now() - lastStartLook >= 10000) {
        lastStartLook = Date.now();
        if (jobShown()) {
          startShown = true;
        } else if (++startMisses >= 2) {
          look();
          return fail("notstarted", "Magnific did not start this picture. Try again.", "no new picture, spinner or queue entry " + Math.round((Date.now() - pressedAt) / 1000) + " s after the press | " + pageState(busyBefore) + " | floor " + floor + " | feed: " + seen.join(" | ").slice(0, 1200));
        }
      }
      const waited = firstSeen ? Date.now() - firstSeen : 0;
      // Enough new pictures with this prompt on their card (fewer after a minute); or the one
      // new picture, its card showing no prompt, under `blankWhy`. Never a card with another prompt.
      const lone = now.all.length === 1 && !now.matched.length && !blankWhy(now, now.all[0]);
      if (now.matched.length >= want || (now.matched.length && waited > 60000) || lone) {
        // Let the page settle, then look again: a card may have shown its prompt meanwhile.
        await sleep(1500);
        const f = fresh();
        if (f.matched.length) {
          got = f.matched.slice(0, want);
          how = "card shows the prompt";
          break;
        }
        if (f.all.length === 1 && !blankWhy(f, f.all[0])) {
          got = [f.all[0]];
          how = "the only new picture, no prompt on its card";
          break;
        }
      }
      if (Date.now() - lastLook >= 20000) {
        lastLook = Date.now();
        look();
      }
      const alert = warning();
      if (alert) return fail("alert", alert.slice(0, 200));
    }
    if (!got.length) {
      // Out of time: still only this prompt's pictures, or the lone prompt-less one.
      const f = fresh();
      if (f.matched.length) {
        got = f.matched.slice(0, want);
        how = "card shows the prompt, at the end of the wait";
      } else if (f.all.length === 1 && !blankWhy(f, f.all[0])) {
        got = [f.all[0]];
        how = "the only new picture, no prompt on its card, at the end of the wait";
      }
      look();
      window.__mangavoxPick = candidates(got);
      if (!got.length) return fail("timeout", appeared.size ? "Magnific did not finish this picture in time (the new pictures in the feed were not this prompt's)." : "Magnific did not finish the picture in time.", "before the press: newest " + (Math.max(0, ...[...before].map(Number)) || "none") + ", floor " + floor + " | " + pageState(busyBefore) + " | candidates: " + (candidates([]) || "none").slice(0, 1200) + " | feed: " + seen.join(" | ").slice(0, 1500));
    }
    window.__mangavoxPick = candidates(got);

    // The pictures at full size (the page shows small previews of them).
    const images = [];
    const found = feedPics();
    for (const id of got) {
      const hit = found.get(String(id));
      if (!hit) continue;
      // (`preview=true`, `preview=1`: whichever the page put on its small copy.)
      const full = hit.url.replace(/([&?])preview=[^&]*&?/, "$1").replace(/[&?]$/, "");
      guard();
      const res = await fetch(full);
      if (!res.ok) return fail("site", "The finished picture could not be downloaded.", "status " + res.status);
      const blob = await res.blob();
      const bm = await createImageBitmap(blob);
      const data = await new Promise((done, bad) => {
        const fr = new FileReader();
        fr.onload = () => done(fr.result);
        fr.onerror = () => bad(new Error("could not read the picture"));
        fr.readAsDataURL(blob);
      });
      images.push({ data, w: bm.width, h: bm.height, id: String(id) });
    }
    if (!images.length) return fail("site", "The finished picture was not found on the page.");
    window.__mangavoxLast = Math.max(Number(window.__mangavoxLast || 0), ...got);

    // Leave the page tidy for the next picture (and for the user). Cancelled or out of time
    // while tidying: it stops there, and the pictures already here still go back.
    try {
      for (let i = 0; i < 30; i++) {
        const rm = first(REMOVE, form);
        if (!rm) break;
        press(rm);
        await sleep(400);
      }
      const clear = q("clear-prompt-button");
      if (clear && !clear.disabled) press(clear);
    } catch (e) {
      if (e !== STOPPED && e !== LATE) throw e;
    }
    return { ok: true, images, slot_fallback: slotFallback, model: MODEL, model_fallback: modelFallback, free_label: label.slice(0, 80), pressed_by_app: !pressedAlready, picked: how + " after " + Math.round((Date.now() - pressedAt) / 1000) + "s | candidates: " + String(window.__mangavoxPick || "").slice(0, 1500) };
  };

  try {
    return await run();
  } catch (e) {
    if (e === STOPPED) return fail("cancelled", "stopped");
    if (e === LATE) return fail("timeout", "Magnific did not finish the picture in time.");
    throw e;
  } finally {
    banner.remove();
    if (window.__mangavoxJob === P.token) window.__mangavoxJob = 0;
  }
})(__PARAMS__)

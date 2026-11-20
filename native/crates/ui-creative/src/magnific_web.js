// Makes ONE picture on Magnific's image generator page, as the signed-in user would by hand:
// Nano Banana 2.1, one image, 1K, unlimited mode on; drops the reference pictures in, types the
// prompt, presses Generate, waits for the new picture and returns it. It presses Generate only
// when the page itself says the picture is "Unlimited"; otherwise it stops and says why.
// `P` = {prompt, aspect, refs: [{name, b64}], wait_ms}. Returns {ok: true, data, w, h, id} or
// {ok: false, need: "signin" | "human" | "paid" | "site", why}.
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
    [...document.querySelectorAll("iframe")].some((f) => {
      const r = f.getBoundingClientRect();
      // The page always carries an invisible reCAPTCHA badge (hidden by CSS): only a check the
      // user can see counts.
      const shown = f.checkVisibility ? f.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true }) : getComputedStyle(f).visibility !== "hidden";
      return shown && r.width > 60 && r.height > 40 && /recaptcha|hcaptcha|turnstile|challenges\.cloudflare|captcha/i.test(f.src || "");
    });

  // The generator's form (the page is an app that loads after the document does).
  const form = await until(() => q("image-generator-form"), 30000, 500);
  if (human()) return fail("human", "Magnific is asking to confirm you are a person.");
  if (!form) {
    const out = [...document.querySelectorAll("a,button")].some((e) => /^(log in|sign up|sign in)$/i.test(text(e)));
    return fail(out ? "signin" : "site", out ? "You are not signed in to Magnific." : "The image generator did not load (" + location.pathname + ").");
  }

  // Signed out, the page still shows the form (with Log in / Sign up at the top) but no offer.
  const visible = (e) => e.getBoundingClientRect().width > 0;
  if ([...document.querySelectorAll("a,button")].some((e) => visible(e) && /^(log in|sign in)$/i.test(text(e)))) return fail("signin", "You are not signed in to Magnific.");

  // Model: Google Nano Banana 2.1.
  const MODEL = "Google Nano Banana 2.1";
  // The form shows before its model menu has loaded.
  const trigger = await until(() => q("tti-mode-selector-v3-trigger"), 20000, 500);
  if (!trigger) return fail("site", "The model menu was not found (" + location.pathname + ").");
  if (text(trigger) !== MODEL) {
    trigger.click();
    const opt = await until(() => q("tti-model-selector-popover-model-imagen-nano-banana-2-1"), 6000);
    if (!opt) {
      await escape();
      return fail("site", MODEL + " is not in the model list.");
    }
    opt.click();
    await until(() => text(q("tti-mode-selector-v3-trigger")) === MODEL, 5000);
    await escape();
    if (text(q("tti-mode-selector-v3-trigger")) !== MODEL) return fail("site", "Could not pick " + MODEL + ".");
  }

  // One picture at a time.
  for (let i = 0; i < 8 && text(q("number-images-value")) !== "1"; i++) {
    const less = q("decrease-number-images-button");
    if (!less || less.disabled) break;
    less.click();
    await sleep(250);
  }

  // The frame.
  const ratio = q("image-aspect-ratio-input");
  if (ratio && P.aspect && text(ratio) !== P.aspect) {
    ratio.click();
    const opt = await until(() => q("image-generator-aspect-ratio-" + P.aspect), 5000);
    if (opt) {
      opt.click();
      await sleep(500);
    }
    await escape();
  }

  // 1K: the only size the unlimited offer covers.
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

  // Unlimited mode on, the prompt rewriter off (the prompt is used as written).
  const unlimited = q("unlimited-mode-toggle-button");
  if (unlimited && text(unlimited) !== "ON") {
    unlimited.click();
    await until(() => text(q("unlimited-mode-toggle-button")) === "ON", 4000);
    await escape();
  }
  const smart = q("smart-prompt-toggle");
  if (smart && smart.getAttribute("aria-checked") === "true") {
    smart.click();
    await sleep(300);
  }

  // References: out with whatever is there, in with this picture's own.
  const box = q("image-references-input");
  const count = () => parseInt((text(box).match(/(\d+)\s*\/\s*\d+/) || [0, "0"])[1], 10);
  for (let i = 0; i < 20 && box; i++) {
    const rm = q("reference-card-remove-button", box);
    if (!rm) break;
    rm.click();
    await sleep(500);
  }
  for (const r of P.refs || []) {
    const drop = q("reference-add-button");
    if (!drop) break;
    const bytes = Uint8Array.from(atob(r.b64), (c) => c.charCodeAt(0));
    const dt = new DataTransfer();
    dt.items.add(new File([bytes], r.name, { type: "image/jpeg" }));
    const before = count();
    for (const type of ["dragenter", "dragover", "drop"]) {
      drop.dispatchEvent(new DragEvent(type, { bubbles: true, cancelable: true, dataTransfer: dt }));
      await sleep(150);
    }
    if (!(await until(() => count() > before, 45000, 500))) return fail("site", "A reference picture did not upload (" + r.name + ").");
  }

  // The prompt.
  const ed = (q("image-prompt-input") || document).querySelector('[role="textbox"]');
  if (!ed) return fail("site", "The prompt box was not found.");
  ed.focus();
  window.getSelection().selectAllChildren(ed);
  document.execCommand("insertText", false, P.prompt);
  await sleep(900);

  // Pressed only when the page says this picture costs nothing.
  // The button comes alive once the prompt is in and the references have finished uploading;
  // its own label then says what the picture costs ("Generate Unlimited", or a credit count).
  const go = await until(() => {
    const b = q("image-generate-button");
    return b && !b.disabled ? b : null;
  }, 90000, 500);
  const label = text(go || q("image-generate-button"));
  const state = "size " + text(q("image-output-axes-trigger")) + ", images " + text(q("number-images-value")) + ", unlimited mode " + text(q("unlimited-mode-toggle-button")) + ", button: " + label.slice(0, 60);
  if (!go) return fail("site", "The Generate button did not become ready (" + state + ").");
  // "Generate Unlimited", or, once the fast allowance is used, "Generate Relax mode", which the page
  // itself calls "Credit-free generations" (slower, still free).
  const relaxFree = /relax mode/i.test(label) && [...document.querySelectorAll("body *")].some((e) => { const t = text(e); return t.length < 120 && /relax mode/i.test(t) && /credit-free/i.test(t); });
  if (text(q("tti-mode-selector-v3-trigger")) !== MODEL || text(q("image-output-axes-trigger")) !== "1K" || text(q("number-images-value")) !== "1" || text(q("unlimited-mode-toggle-button")) !== "ON" || !(/unlimited/i.test(label) || relaxFree)) {
    return fail("paid", "Magnific does not show this picture as Unlimited (" + state + "), so nothing was generated.");
  }

  const ids = () => [...document.querySelectorAll('img[src*="/production/"]')].map((i) => (i.src.match(/production\/(\d+)\//) || [])[1]).filter(Boolean);
  // Picture ids only grow. The last run's picture can show up on the page after this run began
  // (it was then taken for this one, one picture behind): only an id above every one seen before,
  // and above the last one this routine returned, is this run's.
  const floor = Math.max(Number(window.__mangavoxLast || 0), ...ids().map(Number), 0);
  go.click();

  // One picture is made at a time: wait for it before anything else is asked for.
  const end = Date.now() + (P.wait_ms || 300000);
  let id = null;
  while (Date.now() < end) {
    await sleep(2000);
    if (human()) return fail("human", "Magnific is asking to confirm you are a person.");
    const fresh = ids().map(Number).filter((x) => x > floor);
    if (fresh.length) {
      // Let the page settle, then take the newest.
      await sleep(1500);
      id = String(Math.max(...ids().map(Number).filter((x) => x > floor)));
      break;
    }
    const alert = [...document.querySelectorAll('[role="alert"]')].map(text).find((t) => t.length > 8 && /error|fail|limit|wrong|try again|not allowed|blocked|violat/i.test(t));
    if (alert) return fail("site", "Magnific said: " + alert.slice(0, 200));
  }
  if (!id) return fail("site", "Magnific did not finish the picture in time.");

  // The picture at full size (the page shows a small preview of it).
  const img = [...document.querySelectorAll('img[src*="/production/' + id + '/"]')][0];
  // (`preview=true`, `preview=1`: whichever the page put on its small copy.)
  const full = img.src.replace(/([&?])preview=[^&]*&?/, "$1").replace(/[&?]$/, "");
  const res = await fetch(full);
  if (!res.ok) return fail("site", "The finished picture could not be downloaded (" + res.status + ").");
  const blob = await res.blob();
  const bm = await createImageBitmap(blob);
  const data = await new Promise((done, bad) => {
    const fr = new FileReader();
    fr.onload = () => done(fr.result);
    fr.onerror = () => bad(new Error("could not read the picture"));
    fr.readAsDataURL(blob);
  });

  // Leave the page tidy for the next picture (and for the user).
  for (let i = 0; i < 20 && box; i++) {
    const rm = q("reference-card-remove-button", box);
    if (!rm) break;
    rm.click();
    await sleep(400);
  }
  const clear = q("clear-prompt-button");
  if (clear && !clear.disabled) clear.click();
  window.__mangavoxLast = Number(id);
  return { ok: true, data, w: bm.width, h: bm.height, id };
})(__PARAMS__)

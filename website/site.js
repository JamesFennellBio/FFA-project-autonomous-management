// Site behaviour: Farm/Lab switch, top nav, scroll fade-ins, demo widget and edit mode.
// Edit mode changes the page itself; "Export page" downloads it as index.html.
(() => {
  "use strict";

  const root = document.documentElement;
  const page = document.getElementById("page");
  const gnav = document.querySelector(".gnav");
  const links = gnav.querySelector(".gnav-links");
  const editBtn = gnav.querySelector("[data-edit]");
  const DRAFT_KEY = "farmlab-site-draft";

  // Snapshot before anything changes the page, so a saved draft is only
  // restored on top of the same page it was made from.
  const base = hash(page.innerHTML + rootState().join("|"));

  const store = {
    get(k) { try { return localStorage.getItem(k); } catch { return null; } },
    set(k, v) { try { localStorage.setItem(k, v); } catch { /* storage off */ } },
    del(k) { try { localStorage.removeItem(k); } catch { /* storage off */ } },
  };

  function hash(s) {
    let h = 5381;
    for (let i = 0; i < s.length; i++) h = (h * 33) ^ s.charCodeAt(i);
    return (h >>> 0).toString(36);
  }
  function rootState() {
    return [root.dataset.mode, root.style.getPropertyValue("--accent-farm"), root.style.getPropertyValue("--accent-lab")];
  }
  const $$ = (sel, el = document) => [...el.querySelectorAll(sel)];
  const h = (tag, attrs = {}, ...kids) => {
    const el = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs)) {
      if (k === "class") el.className = v;
      else if (k.startsWith("on")) el.addEventListener(k.slice(2), v);
      else el.setAttribute(k, v);
    }
    el.append(...kids);
    return el;
  };

  /* ---------- Farm / Lab ---------- */
  function setMode(mode, { url = true } = {}) {
    if (mode !== "farm" && mode !== "lab") return;
    root.dataset.mode = mode;
    $$("[data-set-mode]").forEach((b) => b.setAttribute("aria-checked", String(b.dataset.setMode === mode)));
    if (url) {
      const u = new URL(location.href);
      u.searchParams.set("mode", mode);
      history.replaceState(null, "", u);
    }
    resetDemo();
    buildNav();
    syncSwatches();
    save();
  }
  document.addEventListener("click", (e) => {
    const b = e.target.closest("[data-set-mode]");
    if (b) setMode(b.dataset.setMode);
  });

  /* ---------- Top nav ---------- */
  // Links are made from the sections, so moving or hiding one in edit mode updates the nav.
  function buildNav() {
    links.replaceChildren();
    for (const sec of $$(".sec", page)) {
      if (!editing && (sec.hasAttribute("data-hidden") || (sec.dataset.only && sec.dataset.only !== root.dataset.mode))) continue;
      links.append(h("a", { href: "#" + sec.id }, sec.dataset.nav || sec.id));
    }
  }
  links.addEventListener("click", (e) => {
    if (e.target.closest("a")) setMenu(false);
  });
  const menuBtn = gnav.querySelector(".menu-btn");
  function setMenu(open) {
    gnav.classList.toggle("open", open);
    menuBtn.setAttribute("aria-expanded", String(open));
  }
  menuBtn.addEventListener("click", () => setMenu(!gnav.classList.contains("open")));
  editBtn.addEventListener("click", () => toggleEdit());

  /* ---------- Fade in on scroll ---------- */
  const seen = "IntersectionObserver" in window
    ? new IntersectionObserver((entries) => {
      for (const en of entries) {
        if (en.isIntersecting) { en.target.classList.add("in"); seen.unobserve(en.target); }
      }
    }, { rootMargin: "0px 0px -8% 0px" })
    : null;
  function reveal() {
    if (!seen) return;
    root.classList.add("js");
    for (const el of $$(".sec .wrap > *, .bento > *, .gallery > *, .stats > *", page)) {
      if (el.classList.contains("rv")) continue;
      el.classList.add("rv");
      seen.observe(el);
    }
  }

  /* ---------- Demo widget ---------- */
  const DEMO = {
    farm: { temp: [21.4, 0.25, 14, 32], second: [38, 1, 20, 60], dp: [1, 0] },
    lab: { temp: [37.0, 0.08, 35.5, 38.5], second: [412, 6, 300, 520], dp: [1, 0] },
  };
  let series = [];
  let vals = {};
  function walk(v, [, step, lo, hi]) {
    return Math.min(hi, Math.max(lo, v + (Math.random() - 0.5) * 2 * step));
  }
  function resetDemo() {
    const d = DEMO[root.dataset.mode] || DEMO.farm;
    vals = { temp: d.temp[0], second: d.second[0] };
    series = [];
    let v = d.temp[0];
    for (let i = 0; i < 48; i++) series.push((v = walk(v, [0, d.temp[1] * 2, d.temp[2], d.temp[3]])));
    drawDemo();
  }
  function drawDemo() {
    const d = DEMO[root.dataset.mode] || DEMO.farm;
    const t = document.querySelector('[data-live="temp"]');
    const s = document.querySelector('[data-live="second"]');
    if (t) t.textContent = vals.temp.toFixed(d.dp[0]);
    if (s) s.textContent = vals.second.toFixed(d.dp[1]);
    const line = document.querySelector(".spark-line");
    const area = document.querySelector(".spark-area");
    if (!line || !series.length) return;
    const lo = Math.min(...series), hi = Math.max(...series), span = hi - lo || 1;
    const pts = series.map((v, i) => [(i / (series.length - 1)) * 300, 62 - ((v - lo) / span) * 54]);
    const path = pts.map(([x, y], i) => (i ? "L" : "M") + x.toFixed(1) + " " + y.toFixed(1)).join("");
    line.setAttribute("d", path);
    area.setAttribute("d", path + "L300 70L0 70Z");
  }
  setInterval(() => {
    if (document.hidden) return;
    const d = DEMO[root.dataset.mode] || DEMO.farm;
    vals.temp = walk(vals.temp, d.temp);
    vals.second = walk(vals.second, d.second);
    series.push(vals.temp);
    series.shift();
    drawDemo();
  }, 2000);

  /* ---------- Edit mode ---------- */
  let editing = false;
  let hud = null;
  const ACCENTS = [
    ["Leaf", "#2f9e57"], ["Emerald", "#10b981"], ["Lime", "#65a30d"],
    ["Forest", "#166534"], ["Teal", "#0f766e"], ["Blue", "#0071e3"],
    ["Sky", "#0ea5e9"], ["Indigo", "#4f46e5"], ["Navy", "#1e3a8a"],
  ];

  function toggleEdit(on = !editing) {
    editing = on;
    document.body.classList.toggle("editing", on);
    for (const el of $$("[data-e]", page)) {
      if (on) {
        el.contentEditable = "true";
        el.spellcheck = true;
      } else {
        el.removeAttribute("contenteditable");
        el.removeAttribute("spellcheck");
      }
    }
    if (on) {
      addTools();
      hud = buildHud();
      document.body.append(hud);
      syncSwatches();
    } else {
      $$(".edit-ui").forEach((n) => n.remove());
      hud?.remove();
      hud = null;
      save();
    }
    editBtn.setAttribute("aria-pressed", String(on));
    editBtn.setAttribute("aria-label", on ? "Stop editing" : "Edit page");
    buildNav();
  }
  document.addEventListener("keydown", (e) => {
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "e") {
      e.preventDefault();
      toggleEdit();
    } else if (e.key === "Escape" && editing && !document.querySelector("dialog[open]")) {
      document.activeElement?.blur();
    }
  });

  function addTools() {
    $$(".edit-ui", page).forEach((n) => n.remove());
    for (const win of $$(".sec", page)) {
      const only = h("select", { "aria-label": "Show this section on", title: "Show this section on" },
        h("option", { value: "" }, "Farm + Lab"), h("option", { value: "farm" }, "Farm only"), h("option", { value: "lab" }, "Lab only"));
      only.value = win.dataset.only || "";
      only.addEventListener("change", () => {
        if (only.value) win.dataset.only = only.value;
        else delete win.dataset.only;
        save();
      });
      const hide = h("button", { type: "button", class: "tool" }, win.hasAttribute("data-hidden") ? "Show" : "Hide");
      hide.addEventListener("click", () => {
        win.toggleAttribute("data-hidden");
        hide.textContent = win.hasAttribute("data-hidden") ? "Show" : "Hide";
        save();
      });
      const move = (dir) => {
        const other = dir < 0 ? win.previousElementSibling : win.nextElementSibling;
        if (!other?.classList.contains("sec")) return;
        if (dir < 0) other.before(win); else other.after(win);
        win.scrollIntoView({ behavior: "smooth", block: "start" });
        buildNav();
        save();
      };
      win.append(h("div", { class: "sec-tools edit-ui" },
        h("button", { type: "button", class: "tool", title: "Move up", onclick: () => move(-1) }, "↑"),
        h("button", { type: "button", class: "tool", title: "Move down", onclick: () => move(1) }, "↓"),
        only, hide));
    }
    $$("[data-list] > *", page).forEach(addItemTools);
  }

  function addItemTools(item) {
    item.querySelector(":scope > .item-tools")?.remove();
    const list = item.parentElement;
    const tools = h("div", { class: "item-tools edit-ui" },
      h("button", { type: "button", class: "tool", title: "Move earlier", onclick: () => { item.previousElementSibling?.before(item); save(); } }, "←"),
      h("button", { type: "button", class: "tool", title: "Duplicate", onclick: () => {
        const copy = item.cloneNode(true);
        copy.querySelectorAll(".edit-ui").forEach((n) => n.remove());
        item.after(copy);
        addItemTools(copy);
        save();
      } }, "⧉"),
      h("button", { type: "button", class: "tool", title: "Delete", onclick: () => {
        if (list.children.length <= 1) return toast("Keep at least one. Hide the section instead.");
        item.remove();
        save();
      } }, "✕"));
    tools.contentEditable = "false";
    item.append(tools);
  }

  // Plain-text paste, and Enter finishes a line (Shift+Enter for a new line).
  page.addEventListener("paste", onPaste);
  function onPaste(e) {
    if (!editing || !e.target.closest("[data-e]")) return;
    e.preventDefault();
    document.execCommand("insertText", false, e.clipboardData.getData("text/plain"));
  }
  document.addEventListener("keydown", (e) => {
    if (!editing || e.key !== "Enter" || !e.target.closest?.("[data-e]")) return;
    e.preventDefault();
    if (e.shiftKey) document.execCommand("insertLineBreak");
    else e.target.blur();
  });
  page.addEventListener("input", () => save());

  // In edit mode links don't navigate; a chip under the focused link edits its address.
  let chip = null;
  document.addEventListener("click", (e) => {
    if (!editing) return;
    const a = e.target.closest("#page a");
    if (a) e.preventDefault();
    const img = e.target.closest("#page [data-img]");
    if (img && !e.target.closest(".edit-ui")) imageSheet(img);
  }, true);
  page.addEventListener("focusin", (e) => {
    const a = editing && e.target.closest("a[data-e]");
    chip?.remove();
    if (!a) return;
    const r = a.getBoundingClientRect();
    chip = h("button", { type: "button", class: "tool edit-ui", style: `position:absolute;z-index:65;left:${r.left + scrollX}px;top:${r.bottom + scrollY + 8}px` }, "🔗 Link address…");
    chip.addEventListener("mousedown", (ev) => ev.preventDefault());
    chip.addEventListener("click", () => linkSheet(a));
    document.body.append(chip);
  });
  page.addEventListener("focusout", () => {
    const mine = chip;
    setTimeout(() => {
      // Focus may have moved to another link, which made its own chip.
      if (!mine || mine !== chip || document.activeElement === mine) return;
      mine.remove();
      chip = null;
    }, 150);
  });

  function sheet(title, body, onOk, extra = []) {
    const d = h("dialog", { class: "sheet edit-ui" }, h("h3", {}, title), ...body,
      h("div", { class: "hud-row" }, ...extra,
        h("button", { type: "button", class: "hud-btn", onclick: () => d.close() }, "Cancel"),
        h("button", { type: "button", class: "hud-btn primary", onclick: async () => { if ((await onOk()) !== false) d.close(); } }, "Done")));
    d.addEventListener("close", () => d.remove());
    document.body.append(d);
    d.showModal();
    return d;
  }

  const SAFE_URL = /^(https?:|mailto:|tel:|#)/i;
  function linkSheet(a) {
    const url = h("input", { type: "url", value: a.getAttribute("href") || "", placeholder: "https://… or mailto:… or #section" });
    sheet("Link address", [h("label", {}, "Opens"), url], () => {
      const v = url.value.trim();
      if (!SAFE_URL.test(v)) { toast("Use an https://, mailto:, tel: or #section link"); return false; }
      a.setAttribute("href", v);
      save();
    });
  }

  function imageSheet(box) {
    const cur = box.querySelector("img");
    const file = h("input", { type: "file", accept: "image/*" });
    const url = h("input", { type: "url", placeholder: "or a link, e.g. img/node.jpg", value: cur && !cur.src.startsWith("data:") ? cur.getAttribute("src") : "" });
    const alt = h("input", { type: "text", placeholder: "What's in the photo", value: cur?.alt || "" });
    const d = sheet("Photo", [h("label", {}, "From your computer"), file, h("label", {}, "Or a link"), url, h("label", {}, "Description"), alt], async () => {
      let src = null;
      if (file.files[0]) src = await shrink(file.files[0]);
      else if (url.value.trim()) {
        const v = url.value.trim();
        if (/^(javascript|data):/i.test(v)) { toast("That link can't be used"); return false; }
        src = v;
      }
      if (src) {
        const img = cur || h("img", { alt: "" });
        img.src = src;
        if (!cur) box.prepend(img);
      }
      const now = box.querySelector("img");
      if (now) now.alt = alt.value.trim();
      save();
    }, cur ? [h("button", { type: "button", class: "hud-btn", style: "margin-right:auto", onclick: () => { cur.remove(); save(); d.close(); } }, "Remove")] : []);
  }

  // Photos are stored inside the page, so scale them down first.
  function shrink(f) {
    return new Promise((resolve, reject) => {
      const img = new Image();
      img.onload = () => {
        const k = Math.min(1, 1600 / Math.max(img.width, img.height));
        const c = document.createElement("canvas");
        c.width = Math.round(img.width * k);
        c.height = Math.round(img.height * k);
        c.getContext("2d").drawImage(img, 0, 0, c.width, c.height);
        URL.revokeObjectURL(img.src);
        resolve(c.toDataURL("image/jpeg", 0.85));
      };
      img.onerror = reject;
      img.src = URL.createObjectURL(f);
    });
  }

  // Each mode has its own accent; the panel edits the one being shown.
  function syncSwatches() {
    if (!hud) return;
    const mode = root.dataset.mode;
    const accent = getComputedStyle(root).getPropertyValue("--accent-" + mode).trim().toLowerCase();
    hud.querySelector(".accent-label").textContent = (mode === "lab" ? "Lab" : "Farm") + " colour";
    $$(".swatch[data-c]", hud).forEach((s) => s.setAttribute("aria-pressed", String(s.dataset.c === accent)));
    if (/^#[0-9a-f]{6}$/.test(accent)) hud.querySelector(".swatch-custom input").value = accent;
  }

  function buildHud() {
    const swatches = h("div", { class: "swatches" });
    const pick = (c) => {
      root.style.setProperty("--accent-" + root.dataset.mode, c);
      syncSwatches();
      save();
    };
    for (const [name, c] of ACCENTS) {
      swatches.append(h("button", { type: "button", class: "swatch", title: name, "aria-label": name, "data-c": c, style: `background:${c}`, onclick: () => pick(c) }));
    }
    const custom = h("input", { type: "color", value: "#2f9e57", "aria-label": "Custom colour" });
    custom.addEventListener("input", () => pick(custom.value));
    swatches.append(h("label", { class: "swatch swatch-custom", title: "Custom colour" }, custom));

    const seg = h("div", { class: "seg", role: "radiogroup", "aria-label": "Editing the" },
      h("button", { type: "button", role: "radio", "data-set-mode": "farm", "aria-checked": String(root.dataset.mode === "farm") }, "Farm text"),
      h("button", { type: "button", role: "radio", "data-set-mode": "lab", "aria-checked": String(root.dataset.mode === "lab") }, "Lab text"));

    const panel = h("aside", { class: "hud edit-ui", "aria-label": "Page editor" },
      h("div", { class: "hud-head" }, "Edit page",
        h("button", { type: "button", class: "hud-x", "aria-label": "Stop editing", onclick: () => toggleEdit(false) }, "✕")),
      h("h4", {}, "Editing"), seg,
      h("h4", { class: "accent-label" }, "Colour"), swatches,
      h("p", { class: "hud-hint" }, "Click text to change it, a photo to replace it. Hover a card to copy or delete it. Top left of each section: move, Farm/Lab only, hide."),
      h("p", { class: "hud-hint" }, "Saved in this browser only. Export page → replace website/index.html with the download."),
      h("div", { class: "hud-row" },
        h("button", { type: "button", class: "hud-btn", onclick: resetDraft }, "Undo all"),
        h("button", { type: "button", class: "hud-btn", onclick: () => toggleEdit(false) }, "Done"),
        h("button", { type: "button", class: "hud-btn primary", onclick: exportPage }, "Export page")));
    dragBy(panel, panel.firstChild);
    return panel;
  }

  function dragBy(panel, handle) {
    handle.addEventListener("pointerdown", (e) => {
      if (e.target.closest("button") || innerWidth <= 760) return;
      const r = panel.getBoundingClientRect();
      const dx = e.clientX - r.left, dy = e.clientY - r.top;
      handle.setPointerCapture(e.pointerId);
      const move = (ev) => {
        panel.style.left = Math.max(0, Math.min(innerWidth - r.width, ev.clientX - dx)) + "px";
        panel.style.top = Math.max(0, Math.min(innerHeight - 60, ev.clientY - dy)) + "px";
        panel.style.right = "auto";
      };
      handle.addEventListener("pointermove", move);
      handle.addEventListener("pointerup", () => handle.removeEventListener("pointermove", move), { once: true });
    });
  }

  /* ---------- Saving ---------- */
  function cleanCopy(el) {
    const c = el.cloneNode(true);
    c.querySelectorAll(".edit-ui").forEach((n) => n.remove());
    for (const n of [c, ...c.querySelectorAll("[contenteditable], [spellcheck], .rv")]) {
      n.removeAttribute("contenteditable");
      n.removeAttribute("spellcheck");
      n.classList.remove("rv", "in");
      if (!n.classList.length) n.removeAttribute("class");
    }
    return c;
  }

  let saveTimer = 0;
  function save() {
    if (!editing && !store.get(DRAFT_KEY)) return;
    clearTimeout(saveTimer);
    saveTimer = setTimeout(() => {
      store.set(DRAFT_KEY, JSON.stringify({
        base,
        page: cleanCopy(page).innerHTML,
        root: rootState(),
      }));
    }, 300);
  }

  function restoreDraft() {
    let d;
    try { d = JSON.parse(store.get(DRAFT_KEY) || "null"); } catch { d = null; }
    if (!d || d.base !== base) return false;
    page.innerHTML = d.page;
    const [, farm, lab] = d.root || [];
    if (farm) root.style.setProperty("--accent-farm", farm);
    if (lab) root.style.setProperty("--accent-lab", lab);
    return true;
  }

  function resetDraft() {
    if (!confirm("Undo every change made in this browser since the page was last exported?")) return;
    store.del(DRAFT_KEY);
    const u = new URL(location.href);
    u.searchParams.delete("edit");
    location.replace(u);
  }

  function exportPage() {
    const doc = root.cloneNode(true);
    doc.querySelector("#page").replaceWith(cleanCopy(page));
    doc.querySelectorAll(".edit-ui, .toast").forEach((n) => n.remove());
    doc.querySelector(".gnav-links").replaceChildren();
    doc.querySelector(".gnav").classList.remove("open");
    doc.querySelector("[data-edit]").removeAttribute("aria-pressed");
    doc.querySelector("[data-edit]").setAttribute("aria-label", "Edit page");
    doc.classList.remove("js");
    if (!doc.classList.length) doc.removeAttribute("class");
    doc.querySelector("body").removeAttribute("class");
    const html = "<!doctype html>\n" + doc.outerHTML + "\n";
    const a = h("a", { href: URL.createObjectURL(new Blob([html], { type: "text/html" })), download: "index.html" });
    document.body.append(a);
    a.click();
    a.remove();
    toast("Downloaded index.html. It opens in " + (root.dataset.mode === "lab" ? "Lab" : "Farm") + " mode; ?mode=lab or ?mode=farm in a link picks one.");
  }

  function toast(text, actions = []) {
    document.querySelector(".toast")?.remove();
    const t = h("div", { class: "toast edit-ui", role: "status" }, text, ...actions);
    document.body.append(t);
    setTimeout(() => t.remove(), actions.length ? 9000 : 4500);
  }

  /* ---------- Start ---------- */
  const restored = restoreDraft();
  const params = new URLSearchParams(location.search);
  setMode(params.get("mode") || root.dataset.mode, { url: false });
  reveal();
  if (restored) {
    toast("Your unsaved edits are back.", [
      h("button", { type: "button", onclick: () => toggleEdit(true) }, "Keep editing"),
      h("button", { type: "button", onclick: resetDraft }, "Throw away"),
    ]);
  }
  if (params.has("edit")) toggleEdit(true);
})();

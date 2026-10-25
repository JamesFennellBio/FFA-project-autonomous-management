// Site behaviour: Farm/Lab switch, windows, dock, demo widget and edit mode.
// Edit mode changes the page itself; "Export page" downloads it as index.html.
(() => {
  "use strict";

  const root = document.documentElement;
  const desk = document.getElementById("desk");
  const dock = document.querySelector(".dock");
  const DRAFT_KEY = "farmlab-site-draft";
  const THEME_KEY = "farmlab-site-theme";

  // Snapshot before anything changes the page, so a saved draft is only
  // restored on top of the same page it was made from.
  const base = hash(desk.innerHTML + rootState().join("|"));

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
    return [root.dataset.mode, root.dataset.wall, root.style.getPropertyValue("--accent")];
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
  const svg = (paths) => {
    const t = document.createElement("template");
    t.innerHTML = `<svg viewBox="0 0 24 24" aria-hidden="true">${paths}</svg>`;
    return t.content.firstChild;
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
    save();
  }
  document.addEventListener("click", (e) => {
    const b = e.target.closest("[data-set-mode]");
    if (b) setMode(b.dataset.setMode);
  });

  /* ---------- Light / dark ---------- */
  const savedTheme = store.get(THEME_KEY);
  if (savedTheme) root.dataset.theme = savedTheme;
  function toggleTheme() {
    const dark = root.dataset.theme
      ? root.dataset.theme === "dark"
      : matchMedia("(prefers-color-scheme: dark)").matches;
    root.dataset.theme = dark ? "light" : "dark";
    store.set(THEME_KEY, root.dataset.theme);
  }

  /* ---------- Windows and dock ---------- */
  const DOCK_ICONS = {
    welcome: ["i-green", '<path d="M4 11l8-7 8 7v9h-5v-6H9v6H4z"/>'],
    features: ["i-teal", '<rect x="4" y="4" width="7" height="7" rx="1.5"/><rect x="13" y="4" width="7" height="7" rx="1.5"/><rect x="4" y="13" width="7" height="7" rx="1.5"/><rect x="13" y="13" width="7" height="7" rx="1.5"/>'],
    how: ["i-forest", '<circle cx="5" cy="12" r="2.2"/><circle cx="19" cy="6" r="2.2"/><circle cx="19" cy="18" r="2.2"/><path d="M7 11l10-4M7 13l10 4"/>'],
    recipes: ["i-lime", '<path d="M9 4h6v4a2 2 0 1 1 0 4v4h-4a2 2 0 1 0-4 0H4V10h4a2 2 0 1 0 0-4H4"/>'],
    security: ["i-dark", '<rect x="3" y="5" width="18" height="14" rx="2"/><path d="M7 10l3 2-3 2M12 15h5"/>'],
    hardware: ["i-sage", '<rect x="6" y="6" width="12" height="12" rx="2"/><path d="M9 2v4M15 2v4M9 18v4M15 18v4M2 9h4M2 15h4M18 9h4M18 15h4"/>'],
    photos: ["i-teal", '<rect x="3" y="5" width="18" height="14" rx="2"/><circle cx="9" cy="10" r="1.8"/><path d="M21 16l-5-5-9 8"/>'],
    contact: ["i-green", '<rect x="3" y="5" width="18" height="14" rx="2"/><path d="M3 7l9 6 9-6"/>'],
  };
  const FALLBACK_ICON = ["i-green", '<rect x="4" y="4" width="16" height="16" rx="3"/>'];

  function buildDock() {
    dock.replaceChildren();
    for (const win of $$(".win", desk)) {
      if (win.hasAttribute("data-hidden") && !editing) continue;
      const [cls, paths] = DOCK_ICONS[win.id] || FALLBACK_ICON;
      const label = win.dataset.dock || win.querySelector(".wtitle")?.textContent || win.id;
      const item = h("button", { type: "button", class: "dock-item" + (win.classList.contains("closed") ? " off" : ""), "aria-label": label, "data-for": win.id },
        h("span", { class: "app " + cls }, svg(paths)),
        h("span", { class: "dock-tip" }, label));
      item.addEventListener("click", () => {
        win.classList.remove("closed", "collapsed");
        item.classList.remove("off");
        win.scrollIntoView({ behavior: "smooth", block: "start" });
      });
      dock.append(item);
    }
    // The dock is the only bar: the Farm/Lab switch, light/dark and Edit live here too.
    const mode = root.dataset.mode;
    dock.append(h("span", { class: "dock-sep" }),
      h("div", { class: "seg dock-seg dock-ctl", role: "radiogroup", "aria-label": "Show the site for" },
        h("button", { type: "button", role: "radio", "data-set-mode": "farm", "aria-checked": String(mode === "farm") }, "Farm"),
        h("button", { type: "button", role: "radio", "data-set-mode": "lab", "aria-checked": String(mode === "lab") }, "Lab")),
      h("button", { type: "button", class: "dock-item dock-ctl", "aria-label": "Light or dark", onclick: toggleTheme },
        h("span", { class: "app i-sage" }, svg('<path d="M20 14.5A8 8 0 0 1 9.5 4 8 8 0 1 0 20 14.5z"/>')),
        h("span", { class: "dock-tip" }, "Light / dark")),
      h("button", { type: "button", class: "dock-item dock-ctl", "aria-label": "Edit page", "aria-pressed": String(editing), title: "Edit page (Ctrl/⌘ E)", onclick: () => toggleEdit() },
        h("span", { class: "app i-dark" }, svg('<path d="M4 20h4L19 9l-4-4L4 16v4z"/><path d="M13.5 6.5l4 4"/>')),
        h("span", { class: "dock-tip" }, editing ? "Stop editing" : "Edit page")));
  }

  desk.addEventListener("click", (e) => {
    const light = e.target.closest(".l");
    if (!light) return;
    const win = light.closest(".win");
    if (light.classList.contains("l-close")) {
      win.classList.add("closed");
      dock.querySelector(`[data-for="${win.id}"]`)?.classList.add("off");
    } else if (light.classList.contains("l-min")) {
      win.classList.toggle("collapsed");
    } else {
      win.classList.toggle("zoomed");
    }
  });

  // Dock magnification, mouse only.
  if (matchMedia("(hover: hover) and (pointer: fine)").matches) {
    dock.addEventListener("mousemove", (e) => {
      for (const it of $$(".dock-item", dock)) {
        const r = it.getBoundingClientRect();
        const d = Math.abs(e.clientX - (r.left + r.width / 2));
        it.style.setProperty("--s", (1 + 0.45 * Math.max(0, 1 - d / 110)).toFixed(3));
      }
    });
    dock.addEventListener("mouseleave", () => $$(".dock-item", dock).forEach((it) => it.style.removeProperty("--s")));
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
    ["Forest", "#166534"], ["Sage", "#6b8f71"], ["Teal", "#0f766e"],
  ];

  function toggleEdit(on = !editing) {
    editing = on;
    document.body.classList.toggle("editing", on);
    for (const el of $$("[data-e]", desk)) {
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
    } else {
      $$(".edit-ui").forEach((n) => n.remove());
      hud?.remove();
      hud = null;
      save();
    }
    buildDock();
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
    $$(".edit-ui", desk).forEach((n) => n.remove());
    for (const win of $$(".win", desk)) {
      const only = h("select", { "aria-label": "Show this window on", title: "Show this window on" },
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
        if (!other) return;
        if (dir < 0) other.before(win); else other.after(win);
        win.scrollIntoView({ behavior: "smooth", block: "center" });
        buildDock();
        save();
      };
      win.querySelector(".titlebar").append(h("div", { class: "win-tools edit-ui" },
        h("button", { type: "button", class: "tool", title: "Move up", onclick: () => move(-1) }, "↑"),
        h("button", { type: "button", class: "tool", title: "Move down", onclick: () => move(1) }, "↓"),
        only, hide));
    }
    $$("[data-list] > *", desk).forEach(addItemTools);
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
        if (list.children.length <= 1) return toast("Keep at least one. Hide the window instead.");
        item.remove();
        save();
      } }, "✕"));
    tools.contentEditable = "false";
    item.append(tools);
  }

  // Plain-text paste, and Enter finishes a line (Shift+Enter for a new line).
  desk.addEventListener("paste", onPaste);
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
  desk.addEventListener("input", () => save());

  // In edit mode links don't navigate; a chip under the focused link edits its address.
  let chip = null;
  document.addEventListener("click", (e) => {
    if (!editing) return;
    const a = e.target.closest("#desk a");
    if (a && a.matches("#desk a")) e.preventDefault();
    const img = e.target.closest("#desk [data-img]");
    if (img && !e.target.closest(".edit-ui")) imageSheet(img);
  }, true);
  desk.addEventListener("focusin", (e) => {
    const a = editing && e.target.closest("a[data-e]");
    chip?.remove();
    if (!a) return;
    const r = a.getBoundingClientRect();
    chip = h("button", { type: "button", class: "tool edit-ui", style: `position:absolute;z-index:65;left:${r.left + scrollX}px;top:${r.bottom + scrollY + 8}px` }, "🔗 Link address…");
    chip.addEventListener("mousedown", (ev) => ev.preventDefault());
    chip.addEventListener("click", () => linkSheet(a));
    document.body.append(chip);
  });
  desk.addEventListener("focusout", () => {
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

  function buildHud() {
    const accent = getComputedStyle(root).getPropertyValue("--accent").trim();
    const swatches = h("div", { class: "swatches" });
    const pick = (c) => {
      root.style.setProperty("--accent", c);
      $$(".swatch", swatches).forEach((s) => s.setAttribute("aria-pressed", String(s.dataset.c === c)));
      save();
    };
    for (const [name, c] of ACCENTS) {
      swatches.append(h("button", { type: "button", class: "swatch", title: name, "aria-label": name, "data-c": c, "aria-pressed": String(c === accent), style: `background:${c}`, onclick: () => pick(c) }));
    }
    const custom = h("input", { type: "color", value: /^#[0-9a-f]{6}$/i.test(accent) ? accent : "#2f9e57", "aria-label": "Custom colour" });
    custom.addEventListener("input", () => pick(custom.value));
    swatches.append(h("label", { class: "swatch swatch-custom", title: "Custom colour" }, custom));

    const wall = h("select", { "aria-label": "Wallpaper" },
      h("option", { value: "auto" }, "Auto (Meadow for Farm, Fern for Lab)"),
      h("option", { value: "meadow" }, "Meadow"), h("option", { value: "fern" }, "Fern"), h("option", { value: "night" }, "Night"));
    wall.value = root.dataset.wall || "auto";
    wall.addEventListener("change", () => { root.dataset.wall = wall.value; save(); });

    const seg = h("div", { class: "seg", role: "radiogroup", "aria-label": "Editing the" },
      h("button", { type: "button", role: "radio", "data-set-mode": "farm", "aria-checked": String(root.dataset.mode === "farm") }, "Farm text"),
      h("button", { type: "button", role: "radio", "data-set-mode": "lab", "aria-checked": String(root.dataset.mode === "lab") }, "Lab text"));

    const panel = h("aside", { class: "hud edit-ui", "aria-label": "Page editor" },
      h("div", { class: "hud-head" },
        h("div", { class: "lights" }, h("button", { type: "button", class: "l l-close", "aria-label": "Stop editing", onclick: () => toggleEdit(false) }), h("span", { class: "l l-min" }), h("span", { class: "l l-zoom" })),
        "Edit page"),
      h("h4", {}, "Editing"), seg,
      h("h4", {}, "Accent colour"), swatches,
      h("h4", {}, "Wallpaper"), wall,
      h("p", { class: "hud-hint" }, "Click text to change it, a photo to replace it. Hover a card to copy or delete it. Title bars: move, Farm/Lab only, hide."),
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
    for (const n of [c, ...c.querySelectorAll("[contenteditable], [spellcheck], .win")]) {
      n.removeAttribute("contenteditable");
      n.removeAttribute("spellcheck");
      n.classList?.remove("closed", "collapsed", "zoomed");
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
        desk: cleanCopy(desk).innerHTML,
        root: rootState(),
      }));
    }, 300);
  }

  function restoreDraft() {
    let d;
    try { d = JSON.parse(store.get(DRAFT_KEY) || "null"); } catch { d = null; }
    if (!d || d.base !== base) return false;
    desk.innerHTML = d.desk;
    const [, wall, accent] = d.root || [];
    if (wall) root.dataset.wall = wall;
    if (accent) root.style.setProperty("--accent", accent);
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
    doc.querySelector("#desk").replaceWith(cleanCopy(desk));
    doc.querySelectorAll(".edit-ui, .toast").forEach((n) => n.remove());
    doc.querySelector(".dock").replaceChildren();
    doc.removeAttribute("data-theme");
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
  buildDock();
  if (restored) {
    toast("Your unsaved edits are back.", [
      h("button", { type: "button", onclick: () => toggleEdit(true) }, "Keep editing"),
      h("button", { type: "button", onclick: resetDraft }, "Throw away"),
    ]);
  }
  if (params.has("edit")) toggleEdit(true);
})();

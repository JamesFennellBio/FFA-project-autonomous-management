// The Blockly recipe editor runs in a WebView. This builds its page.
//
// It never makes code: the blocks turn into the hub's recipe JSON (see
// hub-node/src/recipes.rs), which the hub checks again before saving.
//
// RN -> page:  window.FARMLAB_INIT = { sensors, outputs, recipe } (before load)
// page -> RN:  { type: "recipe", when, then, otherwise } or { type: "error", message }
//              after every change.

import { BLOCKLY_JS } from "./blocklyBundle.generated";
import type { Cond, Recipe } from "../hub/types";

export interface EditorChannel {
  node: number;
  channel: number;
  label: string;
  kind: string;
  /** Number shown in the block = stored value / scale (°C vs milli-°C). */
  scale: number;
  unit: string;
}

export interface EditorInit {
  sensors: EditorChannel[];
  outputs: EditorChannel[];
  recipe: Pick<Recipe, "when" | "then" | "otherwise"> | null;
}

export type EditorMessage =
  | { type: "recipe"; when: Cond; then: Recipe["then"]; otherwise: Recipe["otherwise"] }
  | { type: "error"; message: string };

const EDITOR_JS = String.raw`
(function () {
  const init = window.FARMLAB_INIT;
  const post = (m) => window.ReactNativeWebView.postMessage(JSON.stringify(m));
  const key = (n, c) => n + ":" + c;
  const byKey = {};
  for (const s of init.sensors.concat(init.outputs)) byKey[key(s.node, s.channel)] = s;

  // Channels a saved recipe uses that the hub no longer lists still need a
  // dropdown entry, or Blockly would throw the value away.
  const extra = { sensors: [], outputs: [] };
  function remember(list, n, c) {
    const k = key(n, c);
    if (!byKey[k]) {
      byKey[k] = { node: n, channel: c, label: "Node " + n + " ch " + c + " (not reporting)", kind: "unknown", scale: 1, unit: "" };
      extra[list].push(byKey[k]);
    }
  }
  const options = (list, empty) => () => {
    const all = init[list].concat(extra[list]);
    if (all.length === 0) return [[empty, "none"]];
    return all.map((s) => [s.label + (s.unit ? " (" + s.unit + ")" : ""), key(s.node, s.channel)]);
  };

  Blockly.Blocks["farmlab_recipe"] = {
    init() {
      this.appendValueInput("WHEN").setCheck("Cond").appendField("when");
      this.appendStatementInput("THEN").setCheck("Action").appendField("do");
      this.appendStatementInput("ELSE").setCheck("Action").appendField("otherwise");
      this.setColour(120);
      this.setDeletable(false);
      this.setTooltip("Runs 'do' once when the condition becomes true, and 'otherwise' once when it stops being true.");
    },
  };
  Blockly.Blocks["farmlab_compare"] = {
    init() {
      this.appendDummyInput()
        .appendField(new Blockly.FieldDropdown(options("sensors", "(no sensors yet)")), "SENSOR")
        .appendField(new Blockly.FieldDropdown([[">", "gt"], ["<", "lt"], ["≥", "ge"], ["≤", "le"], ["=", "eq"], ["≠", "ne"]]), "OP")
        .appendField(new Blockly.FieldNumber(0), "VALUE");
      this.setOutput(true, "Cond");
      this.setColour(210);
    },
  };
  for (const [type, word] of [["farmlab_and", "and"], ["farmlab_or", "or"]]) {
    Blockly.Blocks[type] = {
      init() {
        this.appendValueInput("A").setCheck("Cond");
        this.appendValueInput("B").setCheck("Cond").appendField(word);
        this.setInputsInline(true);
        this.setOutput(true, "Cond");
        this.setColour(210);
      },
    };
  }
  Blockly.Blocks["farmlab_not"] = {
    init() {
      this.appendValueInput("A").setCheck("Cond").appendField("not");
      this.setOutput(true, "Cond");
      this.setColour(210);
    },
  };
  Blockly.Blocks["farmlab_set"] = {
    init() {
      this.appendDummyInput()
        .appendField("turn")
        .appendField(new Blockly.FieldDropdown(options("outputs", "(no outputs yet)")), "OUTPUT")
        .appendField(new Blockly.FieldDropdown([["on", "1"], ["off", "0"]]), "STATE");
      this.setPreviousStatement(true, "Action");
      this.setNextStatement(true, "Action");
      this.setColour(30);
    },
  };
  Blockly.Blocks["farmlab_alert"] = {
    init() {
      this.appendDummyInput().appendField("send alert").appendField(new Blockly.FieldTextInput("Check the greenhouse"), "TEXT");
      this.setPreviousStatement(true, "Action");
      this.setNextStatement(true, "Action");
      this.setColour(0);
    },
  };

  // On phones Blockly edits text with window.prompt, which a WebView may not
  // have. This small dialog replaces it.
  Blockly.dialog.setPrompt((message, value, done) => {
    const box = document.getElementById("prompt");
    const input = document.getElementById("prompt-input");
    document.getElementById("prompt-label").textContent = message;
    input.value = value;
    box.style.display = "flex";
    input.focus();
    const close = (v) => {
      box.style.display = "none";
      document.getElementById("prompt-ok").onclick = null;
      document.getElementById("prompt-cancel").onclick = null;
      done(v);
    };
    document.getElementById("prompt-ok").onclick = () => close(input.value);
    document.getElementById("prompt-cancel").onclick = () => close(null);
  });

  const workspace = Blockly.inject("blockly", {
    toolbox: {
      kind: "flyoutToolbox",
      contents: ["farmlab_compare", "farmlab_and", "farmlab_or", "farmlab_not", "farmlab_set", "farmlab_alert"].map((type) => ({ kind: "block", type })),
    },
    renderer: "zelos",
    trashcan: true,
    zoom: { controls: true, startScale: 0.8, pinch: true },
    move: { scrollbars: true, drag: true, wheel: false },
  });

  // ---- recipe JSON -> blocks ----
  function condState(c) {
    switch (c.type) {
      case "compare": {
        remember("sensors", c.node, c.channel);
        const s = byKey[key(c.node, c.channel)];
        return { type: "farmlab_compare", fields: { SENSOR: key(c.node, c.channel), OP: c.op, VALUE: c.value / s.scale } };
      }
      case "and": return chain(c.all, "farmlab_and");
      case "or": return chain(c.any, "farmlab_or");
      case "not": return { type: "farmlab_not", inputs: { A: { block: condState(c.cond) } } };
    }
  }
  function chain(list, type) {
    if (list.length === 1) return condState(list[0]);
    return { type, inputs: { A: { block: condState(list[0]) }, B: { block: chain(list.slice(1), type) } } };
  }
  function actionsState(list) {
    let next = null;
    for (let i = list.length - 1; i >= 0; i--) {
      const a = list[i];
      let s;
      if (a.type === "set") {
        remember("outputs", a.node, a.channel);
        s = { type: "farmlab_set", fields: { OUTPUT: key(a.node, a.channel), STATE: a.value ? "1" : "0" } };
      } else {
        s = { type: "farmlab_alert", fields: { TEXT: a.text } };
      }
      if (next) s.next = { block: next };
      next = s;
    }
    return next;
  }

  const top = { type: "farmlab_recipe", x: 20, y: 20, inputs: {} };
  if (init.recipe) {
    top.inputs.WHEN = { block: condState(init.recipe.when) };
    const t = actionsState(init.recipe.then);
    const o = actionsState(init.recipe.otherwise);
    if (t) top.inputs.THEN = { block: t };
    if (o) top.inputs.ELSE = { block: o };
  }
  Blockly.serialization.workspaces.load({ blocks: { languageVersion: 0, blocks: [top] } }, workspace);

  // ---- blocks -> recipe JSON ----
  function sensorOf(b, field) {
    const v = b.getFieldValue(field);
    if (!v || v === "none") throw new Error("Pick a sensor or output in every block.");
    const [node, channel] = v.split(":").map(Number);
    return { node, channel, s: byKey[v] };
  }
  function cond(b) {
    if (!b) throw new Error("Fill every empty slot in the 'when' part.");
    switch (b.type) {
      case "farmlab_compare": {
        const { node, channel, s } = sensorOf(b, "SENSOR");
        const value = Math.round(Number(b.getFieldValue("VALUE")) * (s ? s.scale : 1));
        return { type: "compare", node, channel, op: b.getFieldValue("OP"), value };
      }
      case "farmlab_and":
      case "farmlab_or": {
        const kind = b.type === "farmlab_and" ? "and" : "or";
        const listKey = kind === "and" ? "all" : "any";
        const parts = [];
        for (const name of ["A", "B"]) {
          const c = cond(b.getInputTargetBlock(name));
          // and(and(a, b), c) is saved as and(a, b, c): less nesting for the hub's limit.
          if (c.type === kind) parts.push(...c[listKey]);
          else parts.push(c);
        }
        return { type: kind, [listKey]: parts };
      }
      case "farmlab_not":
        return { type: "not", cond: cond(b.getInputTargetBlock("A")) };
    }
    throw new Error("Unknown block in the 'when' part.");
  }
  function actions(first) {
    const out = [];
    for (let b = first; b; b = b.getNextBlock()) {
      if (b.type === "farmlab_set") {
        const { node, channel } = sensorOf(b, "OUTPUT");
        out.push({ type: "set", node, channel, value: Number(b.getFieldValue("STATE")) });
      } else if (b.type === "farmlab_alert") {
        out.push({ type: "alert", text: String(b.getFieldValue("TEXT")).trim() });
      }
    }
    return out;
  }
  function report() {
    try {
      const r = workspace.getBlocksByType("farmlab_recipe", false)[0];
      const when = cond(r.getInputTargetBlock("WHEN"));
      const then = actions(r.getInputTargetBlock("THEN"));
      const otherwise = actions(r.getInputTargetBlock("ELSE"));
      if (then.length === 0 && otherwise.length === 0) throw new Error("Add something to 'do' or 'otherwise'.");
      post({ type: "recipe", when, then, otherwise });
    } catch (e) {
      post({ type: "error", message: e.message });
    }
  }
  let timer = null;
  workspace.addChangeListener((e) => {
    if (e.isUiEvent) return;
    clearTimeout(timer);
    timer = setTimeout(report, 250);
  });
  report();
})();
`;

/** The full page. `init` is passed separately, before the page loads. */
export function editorHtml(dark: boolean): string {
  const bg = dark ? "#111611" : "#f4f6f3";
  const fg = dark ? "#e6ece5" : "#1b241c";
  return `<!doctype html>
<html><head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1, user-scalable=no">
<style>
  html, body { margin: 0; height: 100%; background: ${bg}; color: ${fg}; font-family: -apple-system, Roboto, sans-serif; }
  #blockly { position: absolute; inset: 0; }
  #prompt { display: none; position: fixed; inset: 0; background: rgba(0,0,0,.45); align-items: center; justify-content: center; z-index: 1000; }
  #prompt > div { background: ${bg}; padding: 16px; border-radius: 12px; width: min(90vw, 360px); display: flex; flex-direction: column; gap: 10px; }
  #prompt input { font-size: 17px; padding: 8px; border-radius: 8px; border: 1px solid #888; background: transparent; color: ${fg}; }
  #prompt .row { display: flex; gap: 8px; justify-content: flex-end; }
  #prompt button { font-size: 16px; padding: 8px 14px; border-radius: 8px; border: 1px solid #888; background: transparent; color: ${fg}; }
</style>
</head><body>
<div id="blockly"></div>
<div id="prompt"><div>
  <label id="prompt-label" for="prompt-input"></label>
  <input id="prompt-input" autocomplete="off">
  <div class="row"><button id="prompt-cancel">Cancel</button><button id="prompt-ok">OK</button></div>
</div></div>
<script>${BLOCKLY_JS}</script>
<script>${EDITOR_JS.replaceAll("</script", "<\\/script")}</script>
</body></html>`;
}

/** Script that sets window.FARMLAB_INIT before the page's own scripts run. */
export function editorInitScript(init: EditorInit): string {
  return `window.FARMLAB_INIT = ${JSON.stringify(init).replaceAll("</", "<\\/")}; true;`;
}

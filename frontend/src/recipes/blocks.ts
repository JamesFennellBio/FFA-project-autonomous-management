// Blockly blocks for hub recipes, and converting between blocks and recipe JSON.
//
// Keep in step with the phone's editor (mobile/app/src/recipes/editorHtml.ts):
// same block types and the same JSON, so a recipe made on one app opens in the
// other. Blocks never make code; the hub checks the JSON again before saving.

import * as Blockly from "blockly";
import type { Action, Channel, Cond, NodeInfo, Recipe } from "../types";

export interface EditorChannel {
  node: number;
  channel: number;
  label: string;
  /** Number shown in the block = stored value / scale (°C vs milli-°C). */
  scale: number;
  unit: string;
}

const key = (n: number, c: number) => `${n}:${c}`;

function editorChannel(n: NodeInfo, c: Channel): EditorChannel {
  const milli = c.kind === "temperature" || c.kind === "analog_in";
  return {
    node: n.id,
    channel: c.channel,
    label: `${n.name} / ${c.name}`,
    scale: milli ? 1000 : 1,
    unit: c.kind === "temperature" ? "°C" : c.kind === "analog_in" ? "V" : "",
  };
}

/** Channel lists the dropdowns offer. Set before a workspace is made. */
const lists = { sensors: [] as EditorChannel[], outputs: [] as EditorChannel[] };
const byKey = new Map<string, EditorChannel>();

export function setChannels(nodes: NodeInfo[]) {
  lists.sensors = nodes.flatMap((n) => n.channels.filter((c) => c.kind !== "digital_out").map((c) => editorChannel(n, c)));
  lists.outputs = nodes.flatMap((n) => n.channels.filter((c) => c.kind === "digital_out").map((c) => editorChannel(n, c)));
  byKey.clear();
  for (const c of [...lists.sensors, ...lists.outputs]) byKey.set(key(c.node, c.channel), c);
}

/** A saved recipe may use a channel no node reports any more; it still needs a dropdown entry. */
function remember(list: "sensors" | "outputs", node: number, channel: number) {
  if (byKey.has(key(node, channel))) return;
  const c = { node, channel, label: `Node ${node} ch ${channel} (not reporting)`, scale: 1, unit: "" };
  lists[list].push(c);
  byKey.set(key(node, channel), c);
}

const options = (list: "sensors" | "outputs", empty: string) => (): Blockly.MenuOption[] =>
  lists[list].length === 0
    ? [[empty, "none"]]
    : lists[list].map((c) => [c.label + (c.unit ? ` (${c.unit})` : ""), key(c.node, c.channel)]);

let defined = false;
function defineBlocks() {
  if (defined) return;
  defined = true;
  Blockly.Blocks["farmlab_recipe"] = {
    init(this: Blockly.Block) {
      this.appendValueInput("WHEN").setCheck("Cond").appendField("when");
      this.appendStatementInput("THEN").setCheck("Action").appendField("do");
      this.appendStatementInput("ELSE").setCheck("Action").appendField("otherwise");
      this.setColour(120);
      this.setDeletable(false);
      this.setTooltip("Runs 'do' once when the condition becomes true, and 'otherwise' once when it stops being true.");
    },
  };
  Blockly.Blocks["farmlab_compare"] = {
    init(this: Blockly.Block) {
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
      init(this: Blockly.Block) {
        this.appendValueInput("A").setCheck("Cond");
        this.appendValueInput("B").setCheck("Cond").appendField(word);
        this.setInputsInline(true);
        this.setOutput(true, "Cond");
        this.setColour(210);
      },
    };
  }
  Blockly.Blocks["farmlab_not"] = {
    init(this: Blockly.Block) {
      this.appendValueInput("A").setCheck("Cond").appendField("not");
      this.setOutput(true, "Cond");
      this.setColour(210);
    },
  };
  Blockly.Blocks["farmlab_set"] = {
    init(this: Blockly.Block) {
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
    init(this: Blockly.Block) {
      this.appendDummyInput().appendField("send alert").appendField(new Blockly.FieldTextInput("Check the greenhouse"), "TEXT");
      this.setPreviousStatement(true, "Action");
      this.setNextStatement(true, "Action");
      this.setColour(0);
    },
  };
}

const TOOLBOX = ["farmlab_compare", "farmlab_and", "farmlab_or", "farmlab_not", "farmlab_set", "farmlab_alert"];

const darkTheme = Blockly.Theme.defineTheme("farmlab-dark", {
  name: "farmlab-dark",
  base: Blockly.Themes.Classic,
  componentStyles: {
    workspaceBackgroundColour: "#1c1c1e",
    toolboxBackgroundColour: "#2c2c2e",
    flyoutBackgroundColour: "#2c2c2e",
    flyoutOpacity: 1,
    scrollbarColour: "#636366",
  },
});
const lightTheme = Blockly.Theme.defineTheme("farmlab-light", {
  name: "farmlab-light",
  base: Blockly.Themes.Classic,
  componentStyles: {
    workspaceBackgroundColour: "#ffffff",
    flyoutBackgroundColour: "#f0f0f3",
    flyoutOpacity: 1,
  },
});

/** Makes the editor in `div`, showing `recipe` (or an empty one). */
export function makeWorkspace(div: HTMLElement, recipe: Pick<Recipe, "when" | "then" | "otherwise"> | null, dark: boolean) {
  defineBlocks();
  const ws = Blockly.inject(div, {
    toolbox: { kind: "flyoutToolbox", contents: TOOLBOX.map((type) => ({ kind: "block", type })) },
    renderer: "zelos",
    theme: dark ? darkTheme : lightTheme,
    media: "blockly-media/",
    sounds: false,
    trashcan: true,
    zoom: { controls: true, wheel: true, startScale: 0.8 },
    move: { scrollbars: true, drag: true, wheel: true },
    grid: { spacing: 24, length: 2, colour: dark ? "#3a3a3c" : "#e5e5ea", snap: false },
  });

  const top: Blockly.serialization.blocks.State = { type: "farmlab_recipe", x: 20, y: 20, inputs: {} };
  if (recipe) {
    top.inputs!.WHEN = { block: condState(recipe.when) };
    const t = actionsState(recipe.then);
    const o = actionsState(recipe.otherwise);
    if (t) top.inputs!.THEN = { block: t };
    if (o) top.inputs!.ELSE = { block: o };
  }
  Blockly.serialization.workspaces.load({ blocks: { languageVersion: 0, blocks: [top] } }, ws);
  return ws;
}

// ---- recipe JSON -> blocks ----
type BlockState = Blockly.serialization.blocks.State;

function condState(c: Cond): BlockState {
  switch (c.type) {
    case "compare": {
      remember("sensors", c.node, c.channel);
      const s = byKey.get(key(c.node, c.channel))!;
      return { type: "farmlab_compare", fields: { SENSOR: key(c.node, c.channel), OP: c.op, VALUE: c.value / s.scale } };
    }
    case "and":
      return chain(c.all, "farmlab_and");
    case "or":
      return chain(c.any, "farmlab_or");
    case "not":
      return { type: "farmlab_not", inputs: { A: { block: condState(c.cond) } } };
  }
}

function chain(list: Cond[], type: string): BlockState {
  if (list.length === 1) return condState(list[0]);
  return { type, inputs: { A: { block: condState(list[0]) }, B: { block: chain(list.slice(1), type) } } };
}

function actionsState(list: Action[]): BlockState | null {
  let next: BlockState | null = null;
  for (let i = list.length - 1; i >= 0; i--) {
    const a = list[i];
    let s: BlockState;
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

// ---- blocks -> recipe JSON ----
function picked(b: Blockly.Block, field: string) {
  const v = b.getFieldValue(field) as string | null;
  if (!v || v === "none") throw new Error("Pick a sensor or output in every block.");
  const [node, channel] = v.split(":").map(Number);
  return { node, channel, c: byKey.get(v) };
}

function cond(b: Blockly.Block | null): Cond {
  if (!b) throw new Error("Fill every empty slot in the 'when' part.");
  switch (b.type) {
    case "farmlab_compare": {
      const { node, channel, c } = picked(b, "SENSOR");
      const value = Math.round(Number(b.getFieldValue("VALUE")) * (c ? c.scale : 1));
      return { type: "compare", node, channel, op: b.getFieldValue("OP"), value };
    }
    case "farmlab_and":
    case "farmlab_or": {
      const parts: Cond[] = [];
      for (const name of ["A", "B"]) {
        const c = cond(b.getInputTargetBlock(name));
        // and(and(a, b), c) is saved as and(a, b, c): less nesting for the hub's limit.
        if (c.type === "and" && b.type === "farmlab_and") parts.push(...c.all);
        else if (c.type === "or" && b.type === "farmlab_or") parts.push(...c.any);
        else parts.push(c);
      }
      return b.type === "farmlab_and" ? { type: "and", all: parts } : { type: "or", any: parts };
    }
    case "farmlab_not":
      return { type: "not", cond: cond(b.getInputTargetBlock("A")) };
  }
  throw new Error("Unknown block in the 'when' part.");
}

function actions(first: Blockly.Block | null): Action[] {
  const out: Action[] = [];
  for (let b = first; b; b = b.getNextBlock()) {
    if (b.type === "farmlab_set") {
      const { node, channel } = picked(b, "OUTPUT");
      out.push({ type: "set", node, channel, value: Number(b.getFieldValue("STATE")) });
    } else if (b.type === "farmlab_alert") {
      out.push({ type: "alert", text: String(b.getFieldValue("TEXT")).trim() });
    }
  }
  return out;
}

/** The recipe the blocks describe, or an error saying what to fix. */
export function readWorkspace(ws: Blockly.WorkspaceSvg): Pick<Recipe, "when" | "then" | "otherwise"> {
  const r = ws.getBlocksByType("farmlab_recipe", false)[0];
  const when = cond(r.getInputTargetBlock("WHEN"));
  const then = actions(r.getInputTargetBlock("THEN"));
  const otherwise = actions(r.getInputTargetBlock("ELSE"));
  if (then.length === 0 && otherwise.length === 0) throw new Error("Add something to 'do' or 'otherwise'.");
  return { when, then, otherwise };
}

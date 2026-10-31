// Blockly glue for the Rust UI (app/ui/src/blockly.rs). This file only
// defines the blocks and draws them. Converting blocks <-> recipe JSON is in
// Rust (app/ui/src/recipe_edit.rs), so it's tested and never runs code.
//
// Same block types as the old editors (frontend/src/recipes/blocks.ts,
// mobile/app/src/recipes/editorHtml.ts), so saved recipes still open.
(function () {
  "use strict";
  let lists = { sensors: [], outputs: [] };
  let colors = { recipe: "#52525b", cond: "#2563eb", set: "#475569", alert: "#b42318" };

  const options = (list, empty) => () =>
    lists[list].length === 0
      ? [[empty, "none"]]
      : lists[list].map((c) => [c.label + (c.unit ? ` (${c.unit})` : ""), `${c.node}:${c.channel}`]);

  function define() {
    // Colours can change per mode, so blocks are (re)defined on every open.
    Blockly.Blocks["farmlab_recipe"] = {
      init() {
        this.appendValueInput("WHEN").setCheck("Cond").appendField("when");
        this.appendStatementInput("THEN").setCheck("Action").appendField("do");
        this.appendStatementInput("ELSE").setCheck("Action").appendField("otherwise");
        this.setColour(colors.recipe);
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
        this.setColour(colors.cond);
      },
    };
    for (const [type, word] of [["farmlab_and", "and"], ["farmlab_or", "or"]]) {
      Blockly.Blocks[type] = {
        init() {
          this.appendValueInput("A").setCheck("Cond");
          this.appendValueInput("B").setCheck("Cond").appendField(word);
          this.setInputsInline(true);
          this.setOutput(true, "Cond");
          this.setColour(colors.cond);
        },
      };
    }
    Blockly.Blocks["farmlab_not"] = {
      init() {
        this.appendValueInput("A").setCheck("Cond").appendField("not");
        this.setOutput(true, "Cond");
        this.setColour(colors.cond);
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
        this.setColour(colors.set);
      },
    };
    Blockly.Blocks["farmlab_alert"] = {
      init() {
        this.appendDummyInput().appendField("send alert").appendField(new Blockly.FieldTextInput("Check the greenhouse"), "TEXT");
        this.setPreviousStatement(true, "Action");
        this.setNextStatement(true, "Action");
        this.setColour(colors.alert);
      },
    };
  }

  const TOOLBOX = ["farmlab_compare", "farmlab_and", "farmlab_or", "farmlab_not", "farmlab_set", "farmlab_alert"];

  function theme(dark) {
    const name = dark ? "farmlab-dark" : "farmlab-light";
    return Blockly.Theme.defineTheme(name, {
      name,
      base: Blockly.Themes.Classic,
      fontStyle: { family: "Geist, system-ui, sans-serif", weight: "500", size: 12 },
      componentStyles: dark
        ? { workspaceBackgroundColour: "#17181a", flyoutBackgroundColour: "#1c1d20", flyoutOpacity: 1, scrollbarColour: "#3a3b3f", toolboxBackgroundColour: "#1c1d20" }
        : { workspaceBackgroundColour: "#fafaf8", flyoutBackgroundColour: "#f1f1ef", flyoutOpacity: 1, scrollbarColour: "#c9c9c4" },
    });
  }

  const open = new Map();
  let next = 1;

  window.farmlabBlockly = {
    /** Draws the editor in `div`. `onChange(json)` gets the workspace JSON after every real change. Returns an id for close(). */
    open(div, stateJson, channelsJson, dark, colorsJson, onChange) {
      const chans = JSON.parse(channelsJson);
      lists = { sensors: chans, outputs: chans.filter((c) => c.output) };
      colors = JSON.parse(colorsJson);
      define();
      const ws = Blockly.inject(div, {
        toolbox: { kind: "flyoutToolbox", contents: TOOLBOX.map((type) => ({ kind: "block", type })) },
        renderer: "zelos",
        theme: theme(dark),
        media: "vendor/blockly/media/",
        sounds: false,
        trashcan: true,
        zoom: { controls: true, wheel: true, startScale: 0.8 },
        move: { scrollbars: true, drag: true, wheel: true },
        grid: { spacing: 20, length: 1, colour: dark ? "#2a2b2f" : "#dcdcd7", snap: false },
      });
      Blockly.serialization.workspaces.load(JSON.parse(stateJson), ws);
      const send = () => onChange(JSON.stringify(Blockly.serialization.workspaces.save(ws)));
      ws.addChangeListener((e) => {
        if (!e.isUiEvent) send();
      });
      send();
      const id = next++;
      open.set(id, ws);
      return id;
    },
    close(id) {
      const ws = open.get(id);
      if (ws) ws.dispose();
      open.delete(id);
    },
  };
})();

import { useEffect, useRef, useState } from "react";
import type { WorkspaceSvg } from "blockly";
import { api, usePoll } from "../api";
import { makeWorkspace, readWorkspace, setChannels } from "../recipes/blocks";
import type { NodeInfo, Recipe } from "../types";

// Recipes are stored and run on the hub, so they keep working with the PC off.
// The phone app edits the same ones.

export default function Recipes() {
  const recipes = usePoll(api.recipes, 5000);
  const nodes = usePoll(api.nodes, 10000);
  const [editing, setEditing] = useState<Recipe | "new" | null>(null);
  const [error, setError] = useState<string | null>(null);

  const open = async (id: number) => {
    setError(null);
    try {
      setEditing((await api.recipe(id)).recipe);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };
  const run = async (f: () => Promise<unknown>) => {
    setError(null);
    try {
      await f();
      recipes.refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const list = recipes.data?.recipes ?? [];
  const full = !!recipes.data && list.length >= recipes.data.max_recipes;

  return (
    <>
      <h1>Automation</h1>
      <p className="muted lead">
        Recipes run on the hub, so they keep working when this PC is off. The phone app edits the same recipes.
      </p>
      {editing && nodes.data ? (
        <RecipeEditor
          nodes={nodes.data}
          recipe={editing === "new" ? null : editing}
          onDone={() => {
            setEditing(null);
            recipes.refresh();
          }}
        />
      ) : (
        <div className="toolbar">
          <button className="primary" onClick={() => setEditing("new")} disabled={!nodes.data?.length || !recipes.data || full}>
            + New recipe
          </button>
          {full && <span className="muted small">The hub holds {recipes.data!.max_recipes} recipes at most.</span>}
        </div>
      )}
      {(error ?? recipes.error) && <div className="banner error">{error ?? recipes.error}</div>}
      {recipes.data && list.length === 0 && !editing && (
        <p className="muted">No recipes yet. Example: when greenhouse temp &gt; 30 °C, turn the fans on, otherwise off.</p>
      )}
      {list.length > 0 && (
        <table className="table">
          <tbody>
            {list.map((r) => (
              <tr key={r.id} className={r.enabled ? "" : "muted"}>
                <td>
                  <b>{r.name}</b>
                </td>
                <td>
                  <label className="switch" title={r.enabled ? "Turn off" : "Turn on"}>
                    <input type="checkbox" checked={r.enabled} onChange={() => run(() => api.setRecipeEnabled(r.id, !r.enabled))} />
                    <span />
                    {r.enabled ? "On" : "Off"}
                  </label>
                </td>
                <td className="right nowrap">
                  <button onClick={() => open(r.id)} disabled={!!editing}>
                    Edit
                  </button>{" "}
                  <button className="danger" onClick={() => confirm(`Delete "${r.name}"?`) && run(() => api.deleteRecipe(r.id))}>
                    Delete
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </>
  );
}

function RecipeEditor({ nodes, recipe, onDone }: { nodes: NodeInfo[]; recipe: Recipe | null; onDone: () => void }) {
  const div = useRef<HTMLDivElement>(null);
  const ws = useRef<WorkspaceSvg | null>(null);
  const [name, setName] = useState(recipe?.name ?? "");
  const [enabled, setEnabled] = useState(recipe?.enabled ?? true);
  const [problem, setProblem] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  // Made once per opened recipe. Later node polls don't rebuild it, so edits aren't lost.
  useEffect(() => {
    setChannels(nodes);
    const dark = matchMedia("(prefers-color-scheme: dark)").matches;
    const w = makeWorkspace(div.current!, recipe, dark);
    ws.current = w;
    const check = () => {
      try {
        readWorkspace(w);
        setProblem(null);
      } catch (e) {
        setProblem(e instanceof Error ? e.message : String(e));
      }
    };
    check();
    w.addChangeListener((e) => {
      if (!e.isUiEvent) check();
    });
    return () => {
      w.dispose();
      ws.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [recipe]);

  const save = async () => {
    setError(null);
    if (!name.trim()) return setError("Give the recipe a name.");
    let blocks;
    try {
      blocks = readWorkspace(ws.current!);
    } catch (e) {
      return setError(e instanceof Error ? e.message : String(e));
    }
    setBusy(true);
    try {
      const body = { name: name.trim(), enabled, ...blocks };
      if (recipe) await api.updateRecipe(recipe.id, body);
      else await api.createRecipe(body);
      onDone();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="card recipe-editor">
      <header className="card-head">
        <input className="title-input" value={name} maxLength={32} placeholder="Recipe name, e.g. Cool the barn" onChange={(e) => setName(e.target.value)} />
        <label className="switch">
          <input type="checkbox" checked={enabled} onChange={(e) => setEnabled(e.target.checked)} />
          <span />
          {enabled ? "On" : "Off"}
        </label>
      </header>
      <div className="blockly" ref={div} />
      <div className="row">
        <span className={problem ? "muted small" : "small ok"}>{problem ?? "Ready to save."}</span>
        <span className="spacer" />
        <button onClick={onDone} disabled={busy}>
          Cancel
        </button>
        <button className="primary" onClick={save} disabled={busy || !!problem}>
          {busy ? "Saving…" : "Save to hub"}
        </button>
      </div>
      {error && <div className="error small">{error}</div>}
    </section>
  );
}

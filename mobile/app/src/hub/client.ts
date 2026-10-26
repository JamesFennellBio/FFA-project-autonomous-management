import type { NativeClient } from "./native";
import type {
  ChannelRow,
  HubEvent,
  Info,
  NodeRow,
  PairRequestRow,
  Recipe,
  RecipeSummary,
} from "./types";

type Paged<K extends string, T> = { [key in K]: T[] } & { next: number | null };

/** Typed requests to the hub. Each call is one encrypted round trip (lists may take a few). */
export class Hub {
  constructor(private readonly native: NativeClient) {}

  private async ask<T>(req: Record<string, unknown>): Promise<T> {
    return JSON.parse(await this.native.request(JSON.stringify(req))) as T;
  }

  /** Fetches every page of a list (replies are kept small enough for one UDP packet). */
  private async all<K extends string, T>(type: string, key: K): Promise<T[]> {
    const out: T[] = [];
    let offset = 0;
    for (let i = 0; i < 50; i++) {
      const r = await this.ask<Paged<K, T>>({ type, offset });
      out.push(...r[key]);
      if (r.next == null) break;
      offset = r.next;
    }
    return out;
  }

  info = () => this.ask<Info>({ type: "info" });
  nodes = () => this.all<"nodes", NodeRow>("nodes", "nodes");
  channels = () => this.all<"channels", ChannelRow>("channels", "channels");

  history = (node: number, channel: number, since: number, points = 48) =>
    this.ask<{ points: [number, number][] }>({ type: "history", node, channel, since, points }).then((r) => r.points);

  events = (before?: number) =>
    this.ask<{ events: HubEvent[]; next: number | null }>({ type: "events", before: before ?? null });

  command = (node: number, channel: number, value: number) =>
    this.ask<{ queued: boolean; node_online: boolean }>({ type: "command", node, channel, value });

  pairRequests = () => this.all<"requests", PairRequestRow>("pair_requests", "requests");

  approveNode = (publicKey: string, name?: string) =>
    this.ask<{ forwarded: boolean }>({ type: "approve_node", public_key: publicKey, name: name || null });

  recipes = () => this.all<"recipes", RecipeSummary>("recipes", "recipes");
  getRecipe = (id: number) => this.ask<{ recipe: Recipe }>({ type: "get_recipe", id }).then((r) => r.recipe);
  saveRecipe = (recipe: Recipe) => this.ask<{ id: number }>({ type: "save_recipe", recipe }).then((r) => r.id);
  deleteRecipe = (id: number) => this.ask({ type: "delete_recipe", id });
  setRecipeEnabled = (id: number, enabled: boolean) => this.ask({ type: "set_recipe_enabled", id, enabled });
}

// Electron main process.
//
// The page (renderer) never sees the API token. It asks this process over
// IPC, and this process adds the token and calls the daemon on localhost.
// That also means no CORS setup is needed on the daemon.

import { app, BrowserWindow, ipcMain, shell } from "electron";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

const API_URL = process.env.FFA_API_URL ?? "http://127.0.0.1:7401";

// Same folder the Rust `directories` crate picks for ("org", "ffa", "ffa-daemon").
function tokenPath(): string {
  if (process.env.FFA_TOKEN_FILE) return process.env.FFA_TOKEN_FILE;
  const home = os.homedir();
  switch (process.platform) {
    case "win32":
      return path.join(process.env.APPDATA ?? path.join(home, "AppData", "Roaming"), "ffa", "ffa-daemon", "config", "api-token");
    case "darwin":
      return path.join(home, "Library", "Application Support", "org.ffa.ffa-daemon", "api-token");
    default:
      return path.join(process.env.XDG_CONFIG_HOME ?? path.join(home, ".config"), "ffa-daemon", "api-token");
  }
}

function readToken(): string | null {
  try {
    return fs.readFileSync(tokenPath(), "utf8").trim();
  } catch {
    return null;
  }
}

type ApiRequest = { method: string; path: string; body?: unknown };
type ApiResponse = { ok: boolean; status: number; data: unknown };

const ALLOWED_METHODS = new Set(["GET", "POST", "PATCH", "PUT", "DELETE"]);

ipcMain.handle("api", async (_e, req: ApiRequest): Promise<ApiResponse> => {
  // Only ever talk to our own API, whatever the renderer asks for.
  if (!ALLOWED_METHODS.has(req.method) || !req.path.startsWith("/api/")) {
    return { ok: false, status: 400, data: { error: "bad request" } };
  }
  const token = readToken();
  if (!token) {
    return { ok: false, status: 0, data: { error: `Daemon token not found at ${tokenPath()}. Is pc-daemon running?` } };
  }
  try {
    const res = await fetch(API_URL + req.path, {
      method: req.method,
      headers: {
        Authorization: `Bearer ${token}`,
        ...(req.body !== undefined ? { "Content-Type": "application/json" } : {}),
      },
      body: req.body !== undefined ? JSON.stringify(req.body) : undefined,
    });
    const text = await res.text();
    let data: unknown = null;
    try {
      data = text ? JSON.parse(text) : null;
    } catch {
      data = { error: text };
    }
    return { ok: res.ok, status: res.status, data };
  } catch (err) {
    return { ok: false, status: 0, data: { error: `Can't reach the daemon at ${API_URL} (${String(err)})` } };
  }
});

function createWindow() {
  const win = new BrowserWindow({
    width: 1280,
    height: 820,
    minWidth: 900,
    minHeight: 600,
    title: "FFA Farm & Lab",
    backgroundColor: "#0f1a14",
    webPreferences: {
      preload: path.join(__dirname, "preload.js"),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  });

  // Open external links in the browser, never inside the app.
  win.webContents.setWindowOpenHandler(({ url }) => {
    void shell.openExternal(url);
    return { action: "deny" };
  });

  const dev = process.env.VITE_DEV_SERVER_URL;
  if (dev) {
    void win.loadURL(dev);
  } else {
    void win.loadFile(path.join(__dirname, "..", "dist", "index.html"));
  }
}

app.whenReady().then(() => {
  createWindow();
  app.on("activate", () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow();
  });
});

app.on("window-all-closed", () => {
  if (process.platform !== "darwin") app.quit();
});

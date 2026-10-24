// Exposes exactly one function to the page: window.ffa.api(...).
import { contextBridge, ipcRenderer } from "electron";

contextBridge.exposeInMainWorld("ffa", {
  api: (method: string, path: string, body?: unknown) => ipcRenderer.invoke("api", { method, path, body }),
});

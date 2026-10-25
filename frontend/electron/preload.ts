// Exposes exactly one function to the page: window.farmlab.api(...).
import { contextBridge, ipcRenderer } from "electron";

contextBridge.exposeInMainWorld("farmlab", {
  api: (method: string, path: string, body?: unknown) => ipcRenderer.invoke("api", { method, path, body }),
});

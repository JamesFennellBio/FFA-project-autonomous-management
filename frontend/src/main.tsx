import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles.css";

// In a plain browser during development, fill in sample data (never in a build).
const ready = import.meta.env.DEV && !window.farmlab ? import("./devSample") : Promise.resolve();

void ready.then(() =>
  ReactDOM.createRoot(document.getElementById("root")!).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  ),
);

import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import App from "./App";
import "./styles.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);

// The window is created hidden (tauri.conf.json) so it never shows the
// unloaded white webview — reveal it once React has actually painted.
requestAnimationFrame(() => {
  requestAnimationFrame(() => {
    const win = getCurrentWindow();
    void win.show().then(() => win.setFocus()).catch(() => {});
  });
});

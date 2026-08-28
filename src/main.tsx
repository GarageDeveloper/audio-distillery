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

// The window starts hidden and is revealed by the Rust side on page
// load (a hidden webview never fires requestAnimationFrame, so timing
// this from JS is unreliable). This is only a redundant net in case
// that path ever misses — setTimeout DOES fire in a hidden webview.
window.setTimeout(() => {
  const win = getCurrentWindow();
  void win.isVisible().then((v) => {
    if (!v) void win.show().catch(() => {});
  });
}, 500);

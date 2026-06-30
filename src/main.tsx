import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import PluginOverlay from "./PluginOverlay";

const isOverlay = new URLSearchParams(window.location.search).has("overlay");

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {isOverlay ? <PluginOverlay /> : <App />}
  </React.StrictMode>,
);

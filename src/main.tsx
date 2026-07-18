import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import PluginOverlay from "./PluginOverlay";

const isOverlay = new URLSearchParams(window.location.search).has("overlay");
const rootElement = document.getElementById("root") as HTMLElement;
const bootElement = document.getElementById("nea-boot");
let bootFinished = false;
let bootFallbackTimer: number | undefined;

function prefersReducedMotion() {
  try {
    return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  } catch {
    return false;
  }
}

function finishBoot() {
  if (bootFinished) return;
  bootFinished = true;
  window.removeEventListener("nea:boot-ready", finishBoot);
  if (bootFallbackTimer !== undefined) window.clearTimeout(bootFallbackTimer);

  if (isOverlay) {
    bootElement?.remove();
    return;
  }

  const revealApp = () => {
    document.documentElement.classList.add("nea-app-ready");
    if (!bootElement?.isConnected) return;

    if (prefersReducedMotion()) {
      bootElement.remove();
      return;
    }

    let removed = false;
    let removeTimer: number | undefined;
    const removeBoot = () => {
      if (removed) return;
      removed = true;
      bootElement.removeEventListener("transitionend", handleTransitionEnd);
      if (removeTimer !== undefined) window.clearTimeout(removeTimer);
      bootElement.remove();
    };
    const handleTransitionEnd = (event: TransitionEvent) => {
      if (event.target === bootElement && event.propertyName === "opacity") removeBoot();
    };

    bootElement.addEventListener("transitionend", handleTransitionEnd);
    removeTimer = window.setTimeout(removeBoot, 480);
  };

  if (prefersReducedMotion()) revealApp();
  else window.requestAnimationFrame(() => window.requestAnimationFrame(revealApp));
}

if (isOverlay) {
  finishBoot();
} else {
  window.addEventListener("nea:boot-ready", finishBoot, { once: true });
  bootFallbackTimer = window.setTimeout(finishBoot, 30_000);
}

ReactDOM.createRoot(rootElement).render(
  <React.StrictMode>
    {isOverlay ? <PluginOverlay /> : <App />}
  </React.StrictMode>,
);

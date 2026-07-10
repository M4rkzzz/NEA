import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";

type SavedAccount = {
  id: string;
  displayName: string;
  uid?: string;
  avatarUrl?: string;
  hasLoginState: boolean;
};

type AppData = {
  config?: {
    overlayVertical?: boolean;
  };
  accounts: SavedAccount[];
  currentLoginUid?: string;
};

function marker(account: SavedAccount, currentLoginUid?: string) {
  if (account.uid && account.uid === currentLoginUid) return "current";
  return account.hasLoginState ? "ready" : "login";
}

export default function PluginOverlay() {
  const [data, setData] = useState<AppData>({ accounts: [] });
  const [busyAccountId, setBusyAccountId] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [failedImages, setFailedImages] = useState<Record<string, string>>({});

  useEffect(() => {
    document.documentElement.classList.add("overlay-root");
    document.body.classList.add("overlay-body");
    return () => {
      document.documentElement.classList.remove("overlay-root");
      document.body.classList.remove("overlay-body");
    };
  }, []);

  async function refresh() {
    const next = await invoke<AppData>("get_app_data");
    setData(next);
  }

  async function act(account: SavedAccount) {
    if (busyAccountId || (account.uid && account.uid === data.currentLoginUid)) return;
    setBusyAccountId(account.id);
    setError("");
    try {
      await invoke("plugin_account_action", { accountId: account.id });
      await refresh();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusyAccountId(null);
    }
  }

  async function dragOverlay() {
    setError("");
    try {
      await invoke("drag_overlay");
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  useEffect(() => {
    refresh().catch(() => undefined);
    const timer = window.setInterval(() => refresh().catch(() => undefined), 30000);
    let disposed = false;
    const unlisteners: Array<() => void> = [];
    listen("app-data-changed", () => refresh().catch(() => undefined)).then((next) => {
      if (disposed) next();
      else unlisteners.push(next);
    }).catch((cause) => {
      if (!disposed) setError(cause instanceof Error ? cause.message : String(cause));
    });
    listen<string>("overlay-drag-error", (event) => setError(event.payload)).then((next) => {
      if (disposed) next();
      else unlisteners.push(next);
    }).catch((cause) => {
      if (!disposed) setError(cause instanceof Error ? cause.message : String(cause));
    });
    return () => {
      disposed = true;
      window.clearInterval(timer);
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, []);

  return (
    <main className="plugin-overlay" data-layout={data.config?.overlayVertical ? "vertical" : "horizontal"} title={error || undefined} aria-busy={busyAccountId !== null}>
      {data.accounts.map((account) => (
        <button className="plugin-avatar" data-state={marker(account, data.currentLoginUid)} key={account.id} onClick={() => void act(account)} disabled={busyAccountId !== null || account.uid === data.currentLoginUid} aria-label={`${account.displayName}：${account.uid === data.currentLoginUid ? "当前登录" : account.hasLoginState ? "快速切换" : "登录一次"}`} title={error || account.displayName}>
          <span className="avatar-fallback">{account.displayName.trim().slice(0, 1).toUpperCase() || "?"}</span>
          {account.avatarUrl && failedImages[account.id] !== account.avatarUrl && <img src={account.avatarUrl} alt="" onError={() => setFailedImages((current) => ({ ...current, [account.id]: account.avatarUrl || "" }))} />}
        </button>
      ))}
      <button className="overlay-drag-handle" onPointerDown={(event) => {
        if (event.button === 0) void dragOverlay();
      }} aria-label="调整浮层位置" title="调整浮层位置">
        <span className="move-icon" aria-hidden="true" />
      </button>
    </main>
  );
}

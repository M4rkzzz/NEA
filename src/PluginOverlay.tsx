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
  accounts: SavedAccount[];
  currentLoginUid?: string;
};

function marker(account: SavedAccount, currentLoginUid?: string) {
  if (account.uid && account.uid === currentLoginUid) return "current";
  return account.hasLoginState ? "ready" : "login";
}

export default function PluginOverlay() {
  const [data, setData] = useState<AppData>({ accounts: [] });

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
    if (account.uid && account.uid === data.currentLoginUid) return;
    await invoke("plugin_account_action", { accountId: account.id });
    await refresh();
  }

  useEffect(() => {
    refresh().catch(() => undefined);
    const timer = window.setInterval(() => refresh().catch(() => undefined), 30000);
    let unlisten: (() => void) | undefined;
    listen("app-data-changed", () => refresh().catch(() => undefined)).then((next) => {
      unlisten = next;
    });
    return () => {
      window.clearInterval(timer);
      unlisten?.();
    };
  }, []);

  return (
    <main className="plugin-overlay">
      {data.accounts.map((account) => (
        <button className="plugin-avatar" data-state={marker(account, data.currentLoginUid)} key={account.id} onClick={() => act(account)}>
          <span className="avatar-fallback">{account.displayName.slice(0, 1).toUpperCase()}</span>
          {account.avatarUrl && <img src={account.avatarUrl} alt="" onError={(event) => (event.currentTarget.style.display = "none")} />}
        </button>
      ))}
    </main>
  );
}

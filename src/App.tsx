import { useEffect, useMemo, useRef, useState, type MouseEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open, save } from "@tauri-apps/plugin-dialog";
import { Minus, Square, Trash2, X } from "lucide-react";
import "./App.css";

type AppConfig = {
  oopzInstallDir?: string;
  oopzExePath?: string;
  roamingDataDir?: string;
  localSandboxDir?: string;
  pluginModeEnabled?: boolean;
  pluginAutostartEnabled?: boolean;
  overlayVertical?: boolean;
};

type SavedAccount = {
  id: string;
  displayName: string;
  uid?: string;
  pid?: string;
  userCommonId?: string;
  maskedPhone?: string;
  avatarUrl?: string;
  loginName?: string;
  note?: string;
  hasSessionSnapshot: boolean;
  hasCredential: boolean;
  hasLoginState: boolean;
  createdAt: string;
  updatedAt: string;
  lastUsedAt?: string;
};

type AppData = {
  config: AppConfig;
  accounts: SavedAccount[];
  currentLoginUid?: string;
};

type OopzPaths = {
  oopzInstallDir: string;
  oopzExePath: string;
  roamingDataDir: string;
  localSandboxDir: string;
  source: string;
  valid: boolean;
  message?: string;
};

type ImportedCandidate = {
  uid: string;
  displayName: string;
  pid?: string;
  userCommonId?: string;
  maskedPhone?: string;
  avatarUrl?: string;
  hasRoamingState: boolean;
  hasLocalState: boolean;
  hasCurrentLogin: boolean;
  canSwitch: boolean;
};

type PluginStatus = {
  pluginModeEnabled: boolean;
  watcherInstalled: boolean;
  watcherRunning: boolean;
  pluginRuntimeRunning: boolean;
  oopzRunning: boolean;
  overlayVisible: boolean;
};

type UpdateStatus = {
  state: "idle" | "checking" | "current" | "downloading" | "installing" | "updated" | "error";
  currentVersion: string;
  availableVersion?: string;
  message: string;
  transferred?: number;
  total?: number;
  percent?: number;
};

type WormholeStatus = {
  state: "preparing" | "waiting" | "connecting" | "transferring" | "importing" | "cancelling" | "cancelled" | "complete" | "error";
  direction: "send" | "receive";
  message: string;
  code?: string;
  transferred?: number;
  total?: number;
};

type FeatureKey = "overview" | "switcher";

function fmtDate(value?: string) {
  if (!value) return "-";
  try {
    return new Intl.DateTimeFormat("zh-CN", {
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
    }).format(new Date(value));
  } catch {
    return value;
  }
}

function accountLabel(account: SavedAccount) {
  return account.userCommonId || account.pid || account.uid || account.loginName || "未绑定标识";
}

function accountInitial(account: SavedAccount) {
  return account.displayName.trim().slice(0, 1).toUpperCase() || "?";
}

function exportTimestamp() {
  const now = new Date();
  const parts = [
    now.getFullYear(),
    String(now.getMonth() + 1).padStart(2, "0"),
    String(now.getDate()).padStart(2, "0"),
  ];
  const time = [
    String(now.getHours()).padStart(2, "0"),
    String(now.getMinutes()).padStart(2, "0"),
    String(now.getSeconds()).padStart(2, "0"),
  ];
  return `${parts.join("-")}_${time.join("-")}`;
}

function safeFileName(value: string) {
  return value.replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_").trim() || "oopz-account";
}

function AccountAvatar({ account, className = "", ready = false }: { account: SavedAccount; className?: string; ready?: boolean }) {
  const src = account.avatarUrl?.trim() || "";
  const [failedSrc, setFailedSrc] = useState("");

  useEffect(() => setFailedSrc(""), [src]);

  return (
    <div className={`avatar-wrap ${className}`.trim()} data-ready={ready} aria-hidden="true">
      <span className="avatar-fallback">{accountInitial(account)}</span>
      {src && failedSrc !== src && <img src={src} alt="" onError={() => setFailedSrc(src)} />}
    </div>
  );
}

function errorMessage(error: unknown) {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  try {
    return JSON.stringify(error);
  } catch {
    return "操作失败，请稍后重试";
  }
}

function App() {
  const [data, setData] = useState<AppData>({ config: {}, accounts: [] });
  const [paths, setPaths] = useState<OopzPaths | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [message, setMessage] = useState("正在初始化...");
  const [busy, setBusy] = useState(false);
  const [searchingOopz, setSearchingOopz] = useState(false);
  const [searchPath, setSearchPath] = useState("");
  const [activeFeature, setActiveFeature] = useState<FeatureKey>("overview");
  const [pluginStatus, setPluginStatus] = useState<PluginStatus | null>(null);
  const [updateStatus, setUpdateStatus] = useState<UpdateStatus | null>(null);
  const [wormholeStatus, setWormholeStatus] = useState<WormholeStatus | null>(null);
  const [quickCode, setQuickCode] = useState("");
  const [receiveCode, setReceiveCode] = useState("");
  const scannedOnceRef = useRef(false);
  const dataSignatureRef = useRef("");
  const busyRef = useRef(false);

  const selected = useMemo(
    () => data.accounts.find((account) => account.id === selectedId) || data.accounts[0],
    [data.accounts, selectedId],
  );

  const sessionCount = data.accounts.filter((account) => account.hasLoginState).length;
  const updateActive = updateStatus?.state === "checking" || updateStatus?.state === "downloading" || updateStatus?.state === "installing";
  const wormholeActive = Boolean(wormholeStatus && !["cancelled", "complete", "error"].includes(wormholeStatus.state));

  async function refresh() {
    const next = await invoke<AppData>("get_app_data");
    const nextSignature = JSON.stringify(next);
    if (nextSignature !== dataSignatureRef.current) {
      dataSignatureRef.current = nextSignature;
      setData(next);
    }
    setSelectedId((current) =>
      current && next.accounts.some((account) => account.id === current)
        ? current
        : next.accounts[0]?.id ?? null,
    );
    return next;
  }

  async function runTask<T>(label: string, task: () => Promise<T>) {
    if (busyRef.current) throw new Error("已有操作正在进行，请稍候");
    busyRef.current = true;
    setBusy(true);
    setMessage(label);
    await new Promise((resolve) => requestAnimationFrame(() => resolve(undefined)));
    try {
      const result = await task();
      await refresh();
      return result;
    } catch (error) {
      setMessage(errorMessage(error));
      throw error;
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  }

  function handleAction(action: () => Promise<unknown>) {
    void action().catch((error) => setMessage(errorMessage(error)));
  }

  async function discover() {
    if (busyRef.current) {
      setMessage("已有操作正在进行，请稍候");
      return;
    }
    busyRef.current = true;
    setBusy(true);
    setSearchingOopz(true);
    setSearchPath("");
    setMessage("正在搜索 OOPZ...");
    await new Promise((resolve) => requestAnimationFrame(() => resolve(undefined)));
    try {
      const found = await invoke<OopzPaths>("discover_oopz");
      setPaths(found);
      await refresh();
      setMessage(`已识别 OOPZ：${found.oopzInstallDir}`);
    } catch (error) {
      setMessage(errorMessage(error));
    } finally {
      busyRef.current = false;
      setSearchingOopz(false);
      setBusy(false);
    }
  }

  async function stopDiscover() {
    try {
      await invoke("cancel_oopz_discovery");
      setMessage("正在停止搜索...");
    } catch (error) {
      setMessage(errorMessage(error));
    }
  }

  async function validate() {
    const checked = await runTask("正在校验 OOPZ 路径...", () => invoke<OopzPaths>("validate_configured_paths"));
    setPaths(checked);
    setMessage("路径校验通过");
  }

  async function chooseDir() {
    const dir = await open({ directory: true, multiple: false, title: "选择包含 oopz.exe 的目录" });
    if (!dir || Array.isArray(dir)) return;
    const configured = await runTask("正在保存 OOPZ 目录...", () =>
      invoke<OopzPaths>("set_oopz_directory", { dir }),
    );
    setPaths(configured);
    setMessage("已保存手动指定目录");
  }

  async function refreshAccounts(manual = true) {
    const scanned = await runTask(manual ? "正在刷新账号..." : "正在自动更新账号...", () =>
      invoke<ImportedCandidate[]>("scan_oopz_accounts"),
    );
    scannedOnceRef.current = true;

    const latest = await invoke<AppData>("get_app_data");
    const current = scanned.find((candidate) => candidate.hasCurrentLogin);
    const savedCurrent = current ? latest.accounts.find((account) => account.uid === current.uid) : undefined;
    if (current && (manual || !savedCurrent?.hasLoginState)) {
      const account = await runTask("正在保存当前账号...", () => invoke<SavedAccount>("import_account", { uid: current.uid }));
      setSelectedId(account.id);
      setMessage(`${account.displayName} 的账号数据和头像已更新`);
      return;
    }

    const readyCount = latest.accounts.filter((account) => account.hasLoginState).length;
    setMessage(scanned.length > 0 ? `发现 ${scanned.length} 个账号，${readyCount} 个可快速切换` : "没有发现账号，请先打开 OOPZ 登录一次");
  }

  async function exportSelectedAccount(account: SavedAccount) {
    if (!account.hasLoginState) {
      setMessage("这个账号需要先登录一次，才能导出");
      return;
    }
    const target = await save({
      title: "导出账号登录数据",
      defaultPath: `${safeFileName(account.displayName)}_${exportTimestamp()}.oopz+`,
      filters: [{ name: "OOPZ+ 登录态包", extensions: ["oopz+"] }],
    });
    if (!target) return;
    const count = await runTask("正在导出账号...", () =>
      invoke<number>("export_account_package", { accountId: account.id, path: target }),
    );
    setMessage(`已导出 ${count} 个账号登录态，请妥善保管`);
  }

  async function exportAllAccounts() {
    const target = await save({
      title: "导出全部账号登录态",
      defaultPath: `${exportTimestamp()}.oopz+`,
      filters: [{ name: "OOPZ+ 登录态包", extensions: ["oopz+"] }],
    });
    if (!target) return;
    const count = await runTask("正在打包全部账号...", () =>
      invoke<number>("export_all_accounts_package", { path: target }),
    );
    setMessage(`已将 ${count} 个账号登录态打包导出，请妥善保管`);
  }

  async function importAccountPackage() {
    const source = await open({
      title: "导入账号登录数据",
      multiple: false,
      filters: [
        { name: "OOPZ+ 登录态包", extensions: ["oopz+"] },
        { name: "旧版 OOPZ+ 登录数据", extensions: ["json", "txt"] },
      ],
    });
    if (!source || Array.isArray(source)) return;
    const accounts = await runTask("正在导入账号...", () =>
      invoke<SavedAccount[]>("import_account_package", { path: source }),
    );
    await refresh();
    setSelectedId(accounts[0]?.id ?? null);
    setMessage(`已导入 ${accounts.length} 个账号，可快速切换`);
  }

  async function startQuickShare() {
    setQuickCode("");
    setWormholeStatus({ state: "preparing", direction: "send", message: "正在准备快捷分享..." });
    try {
      const code = await invoke<string>("start_quick_export");
      setQuickCode(code);
      setMessage("快捷码已生成，等待对方接收");
    } catch (error) {
      const message = errorMessage(error);
      const cancelled = message.includes("已取消");
      setWormholeStatus({ state: cancelled ? "cancelled" : "error", direction: "send", message });
      if (cancelled) setQuickCode("");
      setMessage(message);
    }
  }

  async function cancelQuickShare() {
    try {
      await invoke("cancel_quick_share");
      setWormholeStatus((current) => ({
        state: "cancelling",
        direction: current?.direction || "send",
        message: current?.direction === "receive" ? "正在取消导入..." : "正在取消分享...",
        code: current?.code,
      }));
    } catch (error) {
      setMessage(errorMessage(error));
    }
  }

  async function quickImport() {
    const code = receiveCode.trim();
    if (!code) {
      setMessage("请输入快捷码");
      return;
    }
    setWormholeStatus({ state: "connecting", direction: "receive", message: "正在连接发送方..." });
    const accounts = await runTask("正在快捷导入...", () =>
      invoke<SavedAccount[]>("quick_import", { code }),
    );
    setSelectedId(accounts[0]?.id ?? null);
    setMessage(`快捷导入完成，共 ${accounts.length} 个账号`);
  }

  async function copyText(value?: string) {
    if (!value) {
      setMessage("没有可复制的内容");
      return;
    }
    try {
      await navigator.clipboard.writeText(value);
      setMessage("已复制到剪贴板");
    } catch (error) {
      setMessage(`复制失败：${errorMessage(error)}`);
    }
  }

  async function switchAccount(account: SavedAccount) {
    if (account.hasLoginState) {
      const ok = window.confirm(`将关闭并重启 OOPZ，切换到 ${account.displayName}。继续？`);
      if (!ok) return;
    }
    const result = await runTask("正在切换账号...", () =>
      invoke<{ ok: boolean; message: string }>("switch_account", { accountId: account.id }),
    );
    setMessage(result.message);
  }

  async function quickSwitch(account: SavedAccount) {
    setSelectedId(account.id);
    await switchAccount(account);
  }

  async function deleteSelected(account: SavedAccount) {
    const ok = window.confirm(`确定删除账号“${account.displayName}”吗？`);
    if (!ok) return;
    await runTask("正在删除账号...", () => invoke("delete_account", { accountId: account.id }));
    setSelectedId(null);
    setMessage("账号已删除");
  }

  async function restoreBackup() {
    const ok = window.confirm("将关闭 OOPZ 并恢复最近一次切换前备份。继续？");
    if (!ok) return;
    const result = await runTask("正在恢复备份...", () =>
      invoke<{ ok: boolean; message: string }>("restore_latest_backup"),
    );
    setMessage(result.message);
  }

  async function refreshPluginStatus() {
    const status = await invoke<PluginStatus>("get_plugin_status");
    setPluginStatus(status);
    return status;
  }

  async function togglePluginMode(enabled: boolean) {
    const status = await runTask(enabled ? "正在开启插件模式..." : "正在关闭插件模式...", () =>
      invoke<PluginStatus>("set_plugin_mode", { enabled }),
    );
    setPluginStatus(status);
    setMessage(enabled ? "插件模式已开启" : "插件模式已关闭");
  }

  async function repairPluginEnvironment() {
    const status = await runTask("正在修复插件环境...", () =>
      invoke<PluginStatus>("repair_plugin_environment"),
    );
    setPluginStatus(status);
    setMessage(status.pluginModeEnabled ? "插件环境已修复" : "插件环境已清理");
  }

  async function resetOverlayPosition() {
    await runTask("正在重置浮层位置...", () => invoke("reset_overlay_position"));
    setMessage("浮层位置已恢复默认");
  }

  async function setOverlayLayout(vertical: boolean) {
    await runTask("正在切换浮层排列...", () => invoke("set_overlay_layout", { vertical }));
    setMessage(vertical ? "浮层已切换为竖排" : "浮层已切换为横排");
  }

  async function checkForUpdates() {
    const status = await invoke<UpdateStatus>("check_for_updates");
    setUpdateStatus(status);
    setMessage(status.message);
  }

  function minimizeWindow() {
    void getCurrentWindow().minimize().catch((error) => setMessage(errorMessage(error)));
  }

  function toggleMaximizeWindow() {
    void getCurrentWindow().toggleMaximize().catch((error) => setMessage(errorMessage(error)));
  }

  function closeWindow() {
    void getCurrentWindow().close().catch((error) => setMessage(errorMessage(error)));
  }

  function startWindowDrag(event: MouseEvent<HTMLElement>) {
    if (event.button !== 0 || event.detail > 1 || (event.target as HTMLElement).closest(".window-controls")) return;
    void getCurrentWindow().startDragging().catch((error) => setMessage(errorMessage(error)));
  }

  useEffect(() => {
    refresh()
      .then(() => validate())
      .catch(() => setMessage("未找到 OOPZ，请在概览里手动选择目录"));
    invoke<UpdateStatus>("get_update_status").then((status) => {
      setUpdateStatus(status);
      if (status.state === "updated" || status.state === "error") setMessage(status.message);
    }).catch(() => undefined);
    refreshPluginStatus().catch(() => undefined);

    let disposed = false;
    const unsubs: Array<() => void> = [];
    const keepListener = (promise: Promise<() => void>) => {
      void promise
        .then((unlisten) => disposed ? unlisten() : unsubs.push(unlisten))
        .catch((error) => {
          if (!disposed) setMessage(`事件监听失败：${errorMessage(error)}`);
        });
    };
    keepListener(listen<string>("tray-action", (event) => {
      if (event.payload === "rediscover") discover().catch(() => undefined);
      if (event.payload === "import") refreshAccounts().catch(() => undefined);
    }));
    keepListener(listen<unknown>("switch-finished", (event) => {
      refresh().catch(() => undefined);
      const payload = event.payload as { Ok?: { message?: string }; Err?: string } | string;
      if (typeof payload === "string") setMessage(payload);
      else if (payload?.Ok?.message) setMessage(payload.Ok.message);
      else if (payload?.Err) setMessage(payload.Err);
      else setMessage("托盘操作已完成");
    }));
    keepListener(listen<string>("oopz-discovery-progress", (event) => {
      setSearchPath(event.payload);
      setMessage(`正在搜索：${event.payload}`);
    }));
    keepListener(listen("app-data-changed", () => {
      refresh().catch(() => undefined);
    }));
    keepListener(listen<string>("plugin-environment-finished", (event) => {
      refreshPluginStatus().catch(() => undefined);
      if (event.payload) setMessage(event.payload);
    }));
    keepListener(listen<UpdateStatus>("update-status", (event) => {
      setUpdateStatus(event.payload);
      setMessage(event.payload.message);
    }));
    keepListener(listen<WormholeStatus>("wormhole-status", (event) => {
      setWormholeStatus(event.payload);
      if (event.payload.code) setQuickCode(event.payload.code);
      if (event.payload.state === "cancelled") setQuickCode("");
      setMessage(event.payload.message);
    }));
    return () => {
      disposed = true;
      unsubs.forEach((unsub) => unsub());
    };
  }, []);

  useEffect(() => {
    if (activeFeature === "switcher" && !scannedOnceRef.current) {
      refreshAccounts(false).catch(() => undefined);
    }
    if (activeFeature === "switcher") {
      refreshPluginStatus().catch(() => undefined);
    }
  }, [activeFeature]);

  const overview = (
    <section className="content-stack">
      <div className="panel">
        <div className="panel-title">
          <h2>OOPZ 状态</h2>
        </div>
        <dl className="paths">
          <dt>程序</dt><dd>{paths?.oopzExePath || data.config.oopzExePath || "未设置"}</dd>
          <dt>Roaming</dt><dd>{paths?.roamingDataDir || data.config.roamingDataDir || "未设置"}</dd>
          <dt>Sandbox</dt><dd>{paths?.localSandboxDir || data.config.localSandboxDir || "未设置"}</dd>
        </dl>
        {searchingOopz && <div className="notice">正在搜索：{searchPath || "准备中"}</div>}
        <div className="actions">
          {searchingOopz ? <button onClick={() => handleAction(stopDiscover)}>停止搜索</button> : <button onClick={() => handleAction(discover)} disabled={busy}>自动搜索</button>}
          <button onClick={() => handleAction(chooseDir)} disabled={busy}>手动选择目录</button>
          <button onClick={() => handleAction(validate)} disabled={busy}>重新校验</button>
          <button onClick={() => handleAction(restoreBackup)} disabled={busy}>恢复最近备份</button>
          <button onClick={() => handleAction(checkForUpdates)} disabled={busy || updateActive}>检查更新</button>
        </div>
        {updateStatus?.state === "downloading" && typeof updateStatus.percent === "number" && (
          <div className="update-percent" aria-live="polite">下载进度 <strong>{updateStatus.percent}%</strong></div>
        )}
      </div>

      <div className="summary-grid">
        <div className="metric"><strong>{data.accounts.length}</strong><span>已保存账号</span></div>
        <div className="metric"><strong>{sessionCount}</strong><span>可快速切换</span></div>
        <div className="metric"><strong>{pluginStatus?.pluginModeEnabled ? "已开启" : "未开启"}</strong><span>插件模式</span></div>
      </div>

      <div className="panel">
        <div className="panel-title">
          <h2>最近账号</h2>
        </div>
        {!selected && <div className="empty">还没有保存账号。</div>}
        {selected && (
          <div className="profile profile-inline">
            <AccountAvatar account={selected} className="profile-avatar" />
            <div>
              <h3>{selected.displayName}</h3>
              <p>{accountLabel(selected)}</p>
            </div>
            <button onClick={() => setActiveFeature("switcher")}>管理</button>
          </div>
        )}
      </div>
    </section>
  );

  const switcher = (
    <section className="switcher-grid">
      <div className="content-stack">
        <div className="panel">
          <div className="panel-title">
            <h2>账号列表</h2>
            <div className="actions">
              <button onClick={() => handleAction(importAccountPackage)} disabled={busy}>导入</button>
              <button onClick={() => handleAction(exportAllAccounts)} disabled={busy || sessionCount === 0}>导出全部</button>
              <button onClick={() => handleAction(() => refreshAccounts())} disabled={busy}>刷新</button>
            </div>
          </div>
          <div className="account-list account-list-compact">
            {data.accounts.length === 0 && <div className="empty">暂无账号。先打开 OOPZ 登录一次，再点刷新。</div>}
            {data.accounts.map((account) => (
              <div className="account-row" data-selected={selected?.id === account.id} key={account.id}>
                <div className="account-row-main">
                  <button className="account-main" onClick={() => setSelectedId(account.id)} aria-expanded={selected?.id === account.id}>
                    <AccountAvatar account={account} ready={account.hasLoginState} />
                    <span><strong>{account.displayName}</strong><small>{accountLabel(account)}</small></span>
                  </button>
                  <div className="account-actions">
                    <button className="icon-button danger" onClick={() => handleAction(() => deleteSelected(account))} disabled={busy} aria-label={`删除 ${account.displayName} 的登录态`} title="删除登录态"><Trash2 size={16} strokeWidth={2} /></button>
                    <button onClick={() => handleAction(() => exportSelectedAccount(account))} disabled={busy || !account.hasLoginState}>导出</button>
                    <button className={account.hasLoginState ? "primary" : ""} onClick={() => handleAction(() => quickSwitch(account))} disabled={busy || account.uid === data.currentLoginUid}>{account.uid === data.currentLoginUid ? "当前登录" : account.hasLoginState ? "快速切号" : "登录一次"}</button>
                  </div>
                </div>
                {selected?.id === account.id && (
                  <dl className="account-details">
                    <dt>手机号</dt><dd>{account.maskedPhone || "-"}</dd>
                    <dt>账号 ID</dt><dd>{account.uid || "-"}</dd>
                    <dt>最近切换</dt><dd>{fmtDate(account.lastUsedAt)}</dd>
                  </dl>
                )}
              </div>
            ))}
          </div>
        </div>

        <div className="panel">
          <div className="panel-title">
            <h2>快捷分享</h2>
          </div>
          <div className="quick-transfer">
            <div className="quick-transfer-row">
              <button onClick={() => void startQuickShare()} disabled={busy || wormholeActive || sessionCount === 0}>快捷分享</button>
              {quickCode && <code className="quick-code">{quickCode}</code>}
              {quickCode && <button onClick={() => copyText(quickCode)} disabled={wormholeActive && wormholeStatus?.direction === "receive"}>复制代码</button>}
              {wormholeActive && <button onClick={() => void cancelQuickShare()} disabled={wormholeStatus?.state === "cancelling"}>{wormholeStatus?.direction === "receive" ? "取消导入" : "取消分享"}</button>}
            </div>
            <div className="quick-transfer-row">
              <input value={receiveCode} onChange={(event) => setReceiveCode(event.target.value)} placeholder="输入快捷码" disabled={busy || wormholeActive} />
              <button className="primary" onClick={() => handleAction(quickImport)} disabled={busy || wormholeActive || !receiveCode.trim()}>快捷导入</button>
            </div>
            {wormholeStatus && <div className="quick-transfer-status" data-state={wormholeStatus.state}>{wormholeStatus.message}</div>}
            {wormholeStatus?.total && wormholeStatus.transferred !== undefined && <progress value={wormholeStatus.transferred} max={wormholeStatus.total} />}
          </div>
        </div>
      </div>

      <div className="content-stack">
        <div className="panel">
          <div className="panel-title"><h2>插件模式</h2></div>
          <div className="plugin-toggle-row">
            <div>
              <strong>{pluginStatus?.pluginModeEnabled ? "已开启" : "未开启"}</strong>
              <p>随 OOPZ 显示账号头像浮层</p>
            </div>
            <button className={pluginStatus?.pluginModeEnabled ? "" : "primary"} onClick={() => handleAction(() => togglePluginMode(!pluginStatus?.pluginModeEnabled))} disabled={busy}>{pluginStatus?.pluginModeEnabled ? "关闭" : "开启"}</button>
          </div>
          <div className="plugin-controls">
            <div className="segmented-control" aria-label="浮层排列方式">
              <button data-active={!data.config.overlayVertical} onClick={() => handleAction(() => setOverlayLayout(false))} disabled={busy}>横排</button>
              <button data-active={Boolean(data.config.overlayVertical)} onClick={() => handleAction(() => setOverlayLayout(true))} disabled={busy}>竖排</button>
            </div>
            <button onClick={() => handleAction(repairPluginEnvironment)} disabled={busy}>修复环境</button>
            <button onClick={() => handleAction(resetOverlayPosition)} disabled={busy}>重置位置</button>
          </div>
          <div className="plugin-health">
            <span>守护进程 {pluginStatus?.watcherInstalled ? "已安装" : "未安装"}</span>
            <span>OOPZ {pluginStatus?.oopzRunning ? "运行中" : "未运行"}</span>
            <span>浮层 {pluginStatus?.overlayVisible ? "已显示" : "等待中"}</span>
          </div>
        </div>
      </div>
    </section>
  );

  return (
    <main className="shell">
      <header className="window-titlebar" data-tauri-drag-region onMouseDown={startWindowDrag} onDoubleClick={toggleMaximizeWindow}>
        <div className="window-brand" data-tauri-drag-region>OOPZ+</div>
        <div className="window-controls">
          <button onClick={minimizeWindow} onDoubleClick={(event) => event.stopPropagation()} aria-label="最小化" title="最小化"><Minus size={15} /></button>
          <button onClick={toggleMaximizeWindow} onDoubleClick={(event) => event.stopPropagation()} aria-label="最大化或还原" title="最大化或还原"><Square size={13} /></button>
          <button className="window-close" onClick={closeWindow} onDoubleClick={(event) => event.stopPropagation()} aria-label="关闭" title="关闭"><X size={16} /></button>
        </div>
      </header>

      <div className="app-layout">
        <aside className="sidebar">
          <nav className="feature-list">
            <button data-active={activeFeature === "overview"} onClick={() => setActiveFeature("overview")}><strong>概览</strong></button>
            <button data-active={activeFeature === "switcher"} onClick={() => setActiveFeature("switcher")}><strong>账号切换</strong></button>
          </nav>
        </aside>

        <section className="workspace">
          <header className="topbar">
            <h2>{activeFeature === "overview" ? "概览" : "账号切换"}</h2>
            <div className="status" data-busy={busy}>{busy && <span className="spinner" />}<span>{message}</span></div>
          </header>
          {activeFeature === "overview" ? overview : switcher}
        </section>
      </div>
    </main>
  );
}

export default App;

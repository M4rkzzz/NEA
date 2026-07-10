import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
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

type CredentialView = {
  loginName?: string;
  password?: string;
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
};

type FeatureKey = "overview" | "switcher" | "plugin";

const emptyCredential = {
  displayName: "",
  loginName: "",
  password: "",
  note: "",
};

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

function accountStatus(account: SavedAccount, currentLoginUid?: string) {
  if (account.uid && account.uid === currentLoginUid) return "当前登录";
  return account.hasLoginState ? "可快速切换" : "需要登录一次";
}

function accountInitial(account: SavedAccount) {
  return account.displayName.trim().slice(0, 1).toUpperCase() || "?";
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
  const [credential, setCredential] = useState(emptyCredential);
  const [revealed, setRevealed] = useState<CredentialView | null>(null);
  const [message, setMessage] = useState("正在初始化...");
  const [busy, setBusy] = useState(false);
  const [searchingOopz, setSearchingOopz] = useState(false);
  const [searchPath, setSearchPath] = useState("");
  const [activeFeature, setActiveFeature] = useState<FeatureKey>("overview");
  const [pluginStatus, setPluginStatus] = useState<PluginStatus | null>(null);
  const [updateStatus, setUpdateStatus] = useState<UpdateStatus | null>(null);
  const scannedOnceRef = useRef(false);
  const dataSignatureRef = useRef("");
  const busyRef = useRef(false);

  const selected = useMemo(
    () => data.accounts.find((account) => account.id === selectedId) || data.accounts[0],
    [data.accounts, selectedId],
  );

  const sessionCount = data.accounts.filter((account) => account.hasLoginState).length;
  const credentialCount = data.accounts.filter((account) => account.hasCredential).length;
  const updateActive = updateStatus?.state === "checking" || updateStatus?.state === "downloading" || updateStatus?.state === "installing";

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
      defaultPath: `${account.displayName || "oopz-account"}.oopzplus.json`,
      filters: [{ name: "OOPZ+ 账号登录数据", extensions: ["json", "txt"] }],
    });
    if (!target) return;
    await runTask("正在导出账号...", () =>
      invoke("export_account_package", { accountId: account.id, path: target }),
    );
    setMessage("账号登录数据已导出，请妥善保管");
  }

  async function importAccountPackage() {
    const source = await open({
      title: "导入账号登录数据",
      multiple: false,
      filters: [{ name: "OOPZ+ 账号登录数据", extensions: ["json", "txt"] }],
    });
    if (!source || Array.isArray(source)) return;
    const account = await runTask("正在导入账号...", () =>
      invoke<SavedAccount>("import_account_package", { path: source }),
    );
    await refresh();
    setSelectedId(account.id);
    setMessage(`${account.displayName} 已导入，可快速切换`);
  }

  async function saveCredential() {
    if (!credential.displayName.trim() || !credential.loginName.trim() || !credential.password) {
      setMessage("请填写名称、账号和密码");
      return;
    }
    const account = await runTask("正在保存账号密码...", () =>
      invoke<SavedAccount>("save_manual_credential", {
        input: {
          accountId: selected?.id ?? null,
          displayName: credential.displayName.trim(),
          loginName: credential.loginName.trim(),
          password: credential.password,
          note: credential.note.trim() || null,
        },
      }),
    );
    setSelectedId(account.id);
    setCredential((current) => ({ ...current, password: "" }));
    setMessage("账号密码已保存");
  }

  async function revealCredential(account: SavedAccount) {
    const secret = await runTask("正在读取账号密码...", () =>
      invoke<CredentialView>("reveal_credential", { accountId: account.id }),
    );
    setRevealed(secret);
    setMessage("账号密码已显示，可复制使用");
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
    const ok = window.confirm(`删除 OOPZ+ 中保存的 ${account.displayName}？不会删除 OOPZ 本体数据。`);
    if (!ok) return;
    await runTask("正在删除账号...", () => invoke("delete_account", { accountId: account.id }));
    setSelectedId(null);
    setRevealed(null);
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

  useEffect(() => {
    refresh()
      .then(() => validate())
      .catch(() => setMessage("未找到 OOPZ，请在概览里手动选择目录"));
    invoke<UpdateStatus>("get_update_status").then((status) => {
      setUpdateStatus(status);
      if (status.state === "updated" || status.state === "error") setMessage(status.message);
    }).catch(() => undefined);

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
    return () => {
      disposed = true;
      unsubs.forEach((unsub) => unsub());
    };
  }, []);

  useEffect(() => {
    if (selected) {
      setCredential({
        displayName: selected.displayName,
        loginName: selected.loginName || "",
        password: "",
        note: selected.note || "",
      });
      setRevealed(null);
    }
  }, [selected?.id]);

  useEffect(() => {
    if (activeFeature === "switcher" && !scannedOnceRef.current) {
      refreshAccounts(false).catch(() => undefined);
    }
    if (activeFeature === "plugin") {
      refreshPluginStatus().catch(() => undefined);
    }
  }, [activeFeature]);

  useEffect(() => {
    if (!revealed) return;
    const timer = window.setTimeout(() => setRevealed(null), 30000);
    return () => window.clearTimeout(timer);
  }, [revealed]);

  useEffect(() => {
    setRevealed(null);
  }, [activeFeature]);

  const overview = (
    <section className="content-stack">
      <div className="panel">
        <div className="panel-title">
          <h2>OOPZ 状态</h2>
          <span>{paths?.source || "未校验"}</span>
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
      </div>

      <div className="summary-grid">
        <div className="metric"><strong>{data.accounts.length}</strong><span>已保存账号</span></div>
        <div className="metric"><strong>{sessionCount}</strong><span>可快速切换</span></div>
        <div className="metric"><strong>{credentialCount}</strong><span>已存账号密码</span></div>
      </div>

      <div className="panel">
        <div className="panel-title">
          <h2>最近账号</h2>
          <span>{selected ? fmtDate(selected.lastUsedAt) : "-"}</span>
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
              <button onClick={() => handleAction(() => refreshAccounts())} disabled={busy}>刷新</button>
            </div>
          </div>
          <div className="account-list account-list-compact">
            {data.accounts.length === 0 && <div className="empty">暂无账号。先打开 OOPZ 登录一次，再点刷新。</div>}
            {data.accounts.map((account) => (
              <div className="account-row" data-selected={selected?.id === account.id} key={account.id}>
                <button className="account-main" onClick={() => setSelectedId(account.id)}>
                  <AccountAvatar account={account} ready={account.hasLoginState} />
                  <span><strong>{account.displayName}</strong><small>{accountStatus(account, data.currentLoginUid)}</small></span>
                </button>
                <button className={account.hasLoginState ? "primary" : ""} onClick={() => handleAction(() => quickSwitch(account))} disabled={busy || account.uid === data.currentLoginUid}>{account.uid === data.currentLoginUid ? "当前登录" : account.hasLoginState ? "快速切号" : "登录一次"}</button>
              </div>
            ))}
          </div>
        </div>
      </div>

      <div className="content-stack">
        <div className="panel detail-panel">
          <div className="panel-title">
            <h2>当前账号</h2>
            {selected && <span>{selected.hasLoginState ? "可切号" : "需要登录一次"}</span>}
          </div>
          {!selected && <div className="empty">选择一个账号查看详情。</div>}
          {selected && (
            <>
              <div className="profile">
                <AccountAvatar account={selected} className="profile-avatar" ready={selected.hasLoginState} />
                <div><h3>{selected.displayName}</h3><p>{accountLabel(selected)}</p></div>
              </div>
              <dl className="meta">
                <dt>手机号</dt><dd>{selected.maskedPhone || "-"}</dd>
                <dt>账号ID</dt><dd>{selected.uid || "-"}</dd>
                <dt>最近切换</dt><dd>{fmtDate(selected.lastUsedAt)}</dd>
                <dt>账号密码</dt><dd>{selected.hasCredential ? "已保存" : "未保存"}</dd>
                <dt>状态</dt><dd>{accountStatus(selected, data.currentLoginUid)}</dd>
              </dl>
              {!selected.hasLoginState && <div className="notice">这个账号还不能快速切换。请先在 OOPZ 里登录一次，然后回到这里点刷新。</div>}
              <div className="actions">
                <button className="primary" onClick={() => handleAction(() => switchAccount(selected))} disabled={busy || selected.uid === data.currentLoginUid}>{selected.uid === data.currentLoginUid ? "当前已登录" : selected.hasLoginState ? "切换并重启 OOPZ" : "打开 OOPZ 登录"}</button>
                {selected.hasLoginState && <button onClick={() => handleAction(() => exportSelectedAccount(selected))} disabled={busy}>导出</button>}
                {selected.hasCredential && <button onClick={() => handleAction(() => revealCredential(selected))} disabled={busy}>显示账号密码</button>}
                <button onClick={() => handleAction(() => deleteSelected(selected))} disabled={busy}>删除</button>
              </div>
              {revealed && (
                <div className="secret-box">
                  <button onClick={() => copyText(revealed.loginName)}>复制账号</button>
                  <button onClick={() => copyText(revealed.password)}>复制密码</button>
                  <button onClick={() => setRevealed(null)}>隐藏</button>
                  <code>{revealed.loginName || ""}</code>
                  <code>{revealed.password || ""}</code>
                </div>
              )}
            </>
          )}
        </div>

        <div className="panel">
          <div className="panel-title"><h2>账号密码</h2><span>本机安全保存</span></div>
          <div className="form">
            <label>名称<input value={credential.displayName} onChange={(e) => setCredential({ ...credential, displayName: e.target.value })} /></label>
            <label>账号<input value={credential.loginName} onChange={(e) => setCredential({ ...credential, loginName: e.target.value })} /></label>
            <label>密码<input type="password" value={credential.password} onChange={(e) => setCredential({ ...credential, password: e.target.value })} /></label>
            <label>备注<input value={credential.note} onChange={(e) => setCredential({ ...credential, note: e.target.value })} /></label>
            <button className="primary" onClick={() => handleAction(saveCredential)} disabled={busy}>保存账号密码</button>
          </div>
        </div>
      </div>
    </section>
  );

  const plugin = (
    <section className="content-stack">
      <div className="panel">
        <div className="panel-title"><h2>插件模式</h2><span>{pluginStatus?.pluginModeEnabled ? "已开启" : "已关闭"}</span></div>
        <div className="plugin-toggle-row">
          <div>
            <strong>{pluginStatus?.pluginModeEnabled ? "插件模式已开启" : "插件模式未开启"}</strong>
            <p>开启后只打开 OOPZ 也会自动显示账号头像浮层。</p>
          </div>
          <div className="actions">
            <div className="segmented-control" aria-label="浮层排列方式">
              <button data-active={!data.config.overlayVertical} onClick={() => handleAction(() => setOverlayLayout(false))} disabled={busy}>横排</button>
              <button data-active={Boolean(data.config.overlayVertical)} onClick={() => handleAction(() => setOverlayLayout(true))} disabled={busy}>竖排</button>
            </div>
            <button onClick={() => handleAction(repairPluginEnvironment)} disabled={busy}>修复环境</button>
            <button onClick={() => handleAction(resetOverlayPosition)} disabled={busy}>重置浮层位置</button>
            <button className={pluginStatus?.pluginModeEnabled ? "" : "primary"} onClick={() => handleAction(() => togglePluginMode(!pluginStatus?.pluginModeEnabled))} disabled={busy}>{pluginStatus?.pluginModeEnabled ? "关闭" : "开启"}</button>
          </div>
        </div>
      </div>
      <div className="summary-grid">
        <div className="metric"><strong>{pluginStatus?.watcherInstalled ? "已安装" : "未安装"}</strong><span>守护进程</span></div>
        <div className="metric"><strong>{pluginStatus?.oopzRunning ? "运行中" : "未运行"}</strong><span>OOPZ</span></div>
        <div className="metric"><strong>{pluginStatus?.overlayVisible ? "已显示" : "等待中"}</strong><span>浮层</span></div>
      </div>
    </section>
  );

  return (
    <main className="shell app-layout">
      <aside className="sidebar">
        <div className="brand"><h1>OOPZ+</h1><p>{paths?.valid || data.config.oopzExePath ? "OOPZ 已配置" : "OOPZ 未配置"}</p></div>
        <nav className="feature-list">
          <button data-active={activeFeature === "overview"} onClick={() => setActiveFeature("overview")}><strong>概览</strong></button>
          <button data-active={activeFeature === "switcher"} onClick={() => setActiveFeature("switcher")}><strong>账号切换</strong></button>
          <button data-active={activeFeature === "plugin"} onClick={() => setActiveFeature("plugin")}><strong>插件模式</strong></button>
        </nav>
      </aside>

      <section className="workspace">
        <header className="topbar">
          <div><h2>{activeFeature === "overview" ? "概览" : activeFeature === "switcher" ? "账号切换" : "插件模式"}</h2></div>
          <div className="status" data-busy={busy}>{busy && <span className="spinner" />}<span>{message}</span></div>
        </header>
        {activeFeature === "overview" ? overview : activeFeature === "switcher" ? switcher : plugin}
      </section>
    </main>
  );
}

export default App;

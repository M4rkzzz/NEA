import { useEffect, useMemo, useRef, useState, type MouseEvent, type UIEvent as ReactUIEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open, save } from "@tauri-apps/plugin-dialog";
import { LayoutDashboard, Minus, MoreHorizontal, RefreshCw, Share2, Square, Trash2, UsersRound, X } from "lucide-react";
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
  schemaVersion?: number;
  config: AppConfig;
  accounts: SavedAccount[];
  steam: SteamWorkspace;
  perfectUnavailableAccountIds?: string[];
  currentLoginUid?: string;
};

type SteamAccount = {
  id: string;
  accountName: string;
  displayName: string;
  rememberPassword: boolean;
  mostRecent: boolean;
  userdataCaptured: boolean;
  lastUsedAt?: string;
  note?: string;
};

type SteamWebSession = {
  id: string;
  steamId?: string;
  accountName?: string;
  displayName: string;
  note?: string;
  createdAt: string;
  lastVerifiedAt?: string;
};

type SteamWorkspace = {
  installation?: { installDir: string; executable: string; valid: boolean };
  accounts: SteamAccount[];
  currentAccountId?: string;
  webSessions: SteamWebSession[];
};

type PerfectArenaWorkspace = {
  installation?: { installDir: string; executable: string; valid: boolean };
  accounts: SteamAccount[];
  currentAccountId?: string;
  running: boolean;
};

type PerfectArenaProfile = {
  steamId: string;
  found: boolean;
  nickname?: string;
  avatarUrl?: string;
  score?: number;
  season?: string;
  playerIdentity?: string;
  highRisk?: boolean;
  reputationRequiresVerification?: boolean;
  reputationPoints?: number;
  reputationLevel?: string;
  updatedAt?: string;
};

type SwitchResult = {
  ok: boolean;
  message: string;
};

type SteamBulkImportResult = {
  imported: number;
  failed: number;
  verificationRequiredAccounts: string[];
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

type QuickShareSelection = {
  oopzAccountIds: string[];
  steamWebSessionIds: string[];
  perfectSessionIds: string[];
};

type QuickImportResult = {
  oopzAccounts: SavedAccount[];
  steamWebAccounts: number;
  perfectAccounts: number;
};

type AppKey = "oopz" | "steam" | "perfect";
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

function perfectScoreLabel(score?: number) {
  if (score == null) return "待检测";
  const rank = score <= 1000 ? "D"
    : score <= 1150 ? "C"
      : score <= 1300 ? "C+"
        : score <= 1450 ? "金C+"
          : score <= 1600 ? "B"
            : score <= 1750 ? "B+"
              : score <= 1900 ? "金B+"
                : score <= 2050 ? "A"
                  : score <= 2200 ? "A+"
                    : "金A+";
  return `${rank}${score}`;
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
  const [data, setData] = useState<AppData>({ config: {}, accounts: [], steam: { accounts: [], webSessions: [] } });
  const [paths, setPaths] = useState<OopzPaths | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [message, setMessage] = useState("正在初始化...");
  const [busy, setBusy] = useState(false);
  const [searchingOopz, setSearchingOopz] = useState(false);
  const [searchPath, setSearchPath] = useState("");
  const [activeApp, setActiveApp] = useState<AppKey>("oopz");
  const [activeFeature, setActiveFeature] = useState<FeatureKey>("overview");
  const [pluginStatus, setPluginStatus] = useState<PluginStatus | null>(null);
  const [updateStatus, setUpdateStatus] = useState<UpdateStatus | null>(null);
  const [wormholeStatus, setWormholeStatus] = useState<WormholeStatus | null>(null);
  const [quickCode, setQuickCode] = useState("");
  const [receiveCode, setReceiveCode] = useState("");
  const [pendingDeleteAccount, setPendingDeleteAccount] = useState<SavedAccount | null>(null);
  const [pendingDeleteSteamAccount, setPendingDeleteSteamAccount] = useState<SteamAccount | null>(null);
  const [pendingDeleteSteamWebSession, setPendingDeleteSteamWebSession] = useState<SteamWebSession | null>(null);
  const [showSteamTextImport, setShowSteamTextImport] = useState(false);
  const [steamTextImportDraft, setSteamTextImportDraft] = useState("");
  const [selectedSteamId, setSelectedSteamId] = useState<string | null>(null);
  const [steamNoteDraft, setSteamNoteDraft] = useState("");
  const [selectedSteamWebSessionId, setSelectedSteamWebSessionId] = useState<string | null>(null);
  const [steamWebNoteDraft, setSteamWebNoteDraft] = useState("");
  const [perfectWorkspace, setPerfectWorkspace] = useState<PerfectArenaWorkspace>({ accounts: [], running: false });
  const [perfectProfiles, setPerfectProfiles] = useState<Record<string, PerfectArenaProfile>>({});
  const [perfectSearch, setPerfectSearch] = useState("");
  const [perfectScoreFilter, setPerfectScoreFilter] = useState("all");
  const [perfectPendingOnly, setPerfectPendingOnly] = useState(false);
  const [perfectAvailableOnly, setPerfectAvailableOnly] = useState(false);
  const [perfectMenuSessionId, setPerfectMenuSessionId] = useState<string | null>(null);
  const [showShareCenter, setShowShareCenter] = useState(false);
  const [sharePerfectAvailableOnly, setSharePerfectAvailableOnly] = useState(false);
  const [shareSelection, setShareSelection] = useState<QuickShareSelection>({
    oopzAccountIds: [],
    steamWebSessionIds: [],
    perfectSessionIds: [],
  });
  const scannedOnceRef = useRef(false);
  const dataSignatureRef = useRef("");
  const busyRef = useRef(false);
  const scrollTimersRef = useRef(new Map<HTMLElement, ReturnType<typeof setTimeout>>());

  const selected = useMemo(
    () => data.accounts.find((account) => account.id === selectedId) || data.accounts[0],
    [data.accounts, selectedId],
  );

  const sessionCount = data.accounts.filter((account) => account.hasLoginState).length;
  const updateActive = updateStatus?.state === "checking" || updateStatus?.state === "downloading" || updateStatus?.state === "installing";
  const wormholeActive = Boolean(wormholeStatus && !["cancelled", "complete", "error"].includes(wormholeStatus.state));
  const shareableOopzAccounts = data.accounts.filter((account) => account.hasLoginState);
  const shareableWebSessions = data.steam.webSessions.filter((session) => Boolean(session.steamId));
  const isPerfectShareUsable = (session: SteamWebSession) => {
    if (!session.steamId) return false;
    const profile = perfectProfiles[session.steamId];
    return !profile?.highRisk
      && !profile?.reputationRequiresVerification
      && !data.perfectUnavailableAccountIds?.includes(session.steamId);
  };
  const selectablePerfectSessions = sharePerfectAvailableOnly
    ? shareableWebSessions.filter(isPerfectShareUsable)
    : shareableWebSessions;
  const selectedShareCount = shareSelection.oopzAccountIds.length
    + new Set([...shareSelection.steamWebSessionIds, ...shareSelection.perfectSessionIds]).size;
  const filteredPerfectSessions = useMemo(() => {
    const query = perfectSearch.trim().toLocaleLowerCase();
    return data.steam.webSessions.filter((session) => {
      const profile = session.steamId ? perfectProfiles[session.steamId] : undefined;
      const highRisk = Boolean(profile?.highRisk || profile?.reputationRequiresVerification);
      const unavailable = Boolean(session.steamId && data.perfectUnavailableAccountIds?.includes(session.steamId));
      const pending = !unavailable && (!profile?.found || profile.score == null || !profile.playerIdentity || (!profile.reputationLevel && !highRisk));
      const score = profile?.score;
      const searchable = [session.steamId, session.accountName, session.displayName, session.note, profile?.nickname]
        .filter(Boolean)
        .join(" ")
        .toLocaleLowerCase();
      if (query && !searchable.includes(query)) return false;
      if (perfectPendingOnly && !pending) return false;
      if (perfectAvailableOnly && (highRisk || unavailable)) return false;
      if (perfectScoreFilter === "pending" && score != null) return false;
      if (perfectScoreFilter === "under-1000" && (score == null || score >= 1000)) return false;
      if (perfectScoreFilter === "1000-1499" && (score == null || score < 1000 || score >= 1500)) return false;
      if (perfectScoreFilter === "1500-1999" && (score == null || score < 1500 || score >= 2000)) return false;
      if (perfectScoreFilter === "2000-plus" && (score == null || score < 2000)) return false;
      return true;
    });
  }, [data.perfectUnavailableAccountIds, data.steam.webSessions, perfectAvailableOnly, perfectPendingOnly, perfectProfiles, perfectScoreFilter, perfectSearch]);

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
      defaultPath: `${safeFileName(account.displayName)}_${exportTimestamp()}.nea`,
      filters: [{ name: "NEA 登录态包", extensions: ["nea"] }],
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
      defaultPath: `${exportTimestamp()}.nea`,
      filters: [{ name: "NEA 登录态包", extensions: ["nea"] }],
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
        { name: "NEA 登录态包", extensions: ["nea"] },
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

  function openShareCenter() {
    const webIds = shareableWebSessions.map((session) => session.id);
    setSharePerfectAvailableOnly(false);
    setShareSelection({
      oopzAccountIds: shareableOopzAccounts.map((account) => account.id),
      steamWebSessionIds: webIds,
      perfectSessionIds: webIds,
    });
    setShowShareCenter(true);
  }

  function toggleShareItem(key: keyof QuickShareSelection, id: string, checked: boolean) {
    setShareSelection((current) => ({
      ...current,
      [key]: checked
        ? Array.from(new Set([...current[key], id]))
        : current[key].filter((value) => value !== id),
    }));
  }

  function toggleShareBranch(key: keyof QuickShareSelection, ids: string[], checked: boolean) {
    setShareSelection((current) => ({ ...current, [key]: checked ? ids : [] }));
  }

  function selectAllShareableAccounts() {
    const webIds = shareableWebSessions.map((session) => session.id);
    setShareSelection({
      oopzAccountIds: shareableOopzAccounts.map((account) => account.id),
      steamWebSessionIds: webIds,
      perfectSessionIds: selectablePerfectSessions.map((session) => session.id),
    });
  }

  function setPerfectShareAvailableOnly(checked: boolean) {
    setSharePerfectAvailableOnly(checked);
    if (!checked) return;
    const usableIds = new Set(shareableWebSessions.filter(isPerfectShareUsable).map((session) => session.id));
    setShareSelection((current) => ({
      ...current,
      perfectSessionIds: current.perfectSessionIds.filter((id) => usableIds.has(id)),
    }));
  }

  async function startQuickShare() {
    setQuickCode("");
    setWormholeStatus({ state: "preparing", direction: "send", message: "正在准备快捷分享..." });
    try {
      const code = await invoke<string>("start_quick_export", { selection: shareSelection });
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
    const imported = await runTask("正在快捷导入...", () =>
      invoke<QuickImportResult>("quick_import", { code }),
    );
    setSelectedId(imported.oopzAccounts[0]?.id ?? null);
    setMessage(`快捷导入完成：OOPZ ${imported.oopzAccounts.length} 个、Steam 网页 ${imported.steamWebAccounts} 个、完美平台 ${imported.perfectAccounts} 个`);
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
    await runTask("正在删除账号...", () => invoke("delete_account", { accountId: account.id }));
    setPendingDeleteAccount(null);
    setSelectedId(null);
    setMessage("账号已删除");
  }

  function confirmDeleteSelected() {
    if (!pendingDeleteAccount || busy) return;
    void handleAction(() => deleteSelected(pendingDeleteAccount));
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

  async function discoverSteam() {
    const workspace = await runTask("正在搜索 Steam...", () => invoke<SteamWorkspace>("discover_steam"));
    setData((current) => ({ ...current, steam: workspace }));
    await refreshPerfectArena();
    setMessage("Steam 账号已刷新");
  }

  async function refreshPerfectArena() {
    const [workspace, profiles] = await Promise.all([
      invoke<PerfectArenaWorkspace>("get_perfect_arena_workspace"),
      invoke<PerfectArenaProfile[]>("get_perfect_arena_profiles"),
    ]);
    setPerfectWorkspace(workspace);
    setPerfectProfiles(Object.fromEntries(profiles.map((profile) => [profile.steamId, profile])));
    return workspace;
  }

  async function refreshPerfectProfiles() {
    const profiles = await runTask("正在读取完美玩家资料...", () => invoke<PerfectArenaProfile[]>("get_perfect_arena_profiles"));
    setPerfectProfiles(Object.fromEntries(profiles.map((profile) => [profile.steamId, profile])));
    const found = profiles.filter((profile) => profile.found).length;
    setMessage(`已读取 ${found}/${profiles.length} 个完美账号的公开资料`);
  }

  async function discoverPerfectArena() {
    const workspace = await runTask("正在搜索完美对战平台...", () => invoke<PerfectArenaWorkspace>("discover_perfect_arena"));
    setPerfectWorkspace(workspace);
    setMessage(workspace.installation ? "完美对战平台账号已刷新" : "未找到完美对战平台");
  }

  async function setPerfectAccountUnavailable(session: SteamWebSession, unavailable: boolean) {
    if (!session.steamId) return;
    const accountIds = await runTask(unavailable ? "正在标记不可用账号..." : "正在恢复可用账号...", () =>
      invoke<string[]>("set_perfect_account_unavailable", { steamId: session.steamId, unavailable }),
    );
    setData((current) => ({ ...current, perfectUnavailableAccountIds: accountIds }));
    setPerfectMenuSessionId(null);
    setMessage(unavailable ? "已标记为不可用账号" : "已恢复为可用账号");
  }

  async function switchSteamAccount(account: SteamAccount) {
    const result = await runTask("正在切换 Steam 账号...", () => invoke<SwitchResult>("switch_steam_account", { accountId: account.id }));
    setMessage(result.message);
  }

  async function deleteSteamAccount(account: SteamAccount) {
    await runTask("正在删除 Steam 账号快照...", () => invoke("delete_steam_account", { accountId: account.id }));
    setPendingDeleteSteamAccount(null);
    setMessage("Steam 账号快照已删除");
  }

  function selectSteamAccount(account: SteamAccount) {
    setSelectedSteamId(account.id);
    setSteamNoteDraft(account.note || "");
  }

  async function saveSteamNote(account: SteamAccount) {
    const workspace = await runTask("正在保存 Steam 账号备注...", () => invoke<SteamWorkspace>("set_steam_account_note", { accountId: account.id, note: steamNoteDraft }));
    setData((current) => ({ ...current, steam: workspace }));
    await refreshPerfectArena();
    setMessage("Steam 账号备注已保存");
  }

  async function createSteamWebSession() {
    const workspace = await runTask("正在创建 Steam 网页账号...", () => invoke<SteamWorkspace>("create_steam_web_session"));
    setData((current) => ({ ...current, steam: workspace }));
    const created = workspace.webSessions[workspace.webSessions.length - 1];
    if (created) {
      setSelectedSteamWebSessionId(created.id);
      setSteamWebNoteDraft("");
    }
    setMessage("Steam 网页账号窗口已打开");
  }

  async function importSteamWebAccountsFromText() {
    const accounts: Array<{ account: string; password: string }> = [];
    const lines = steamTextImportDraft.split(/\r?\n/);
    for (let index = 0; index < lines.length; index += 1) {
      const line = lines[index].trim();
      if (!line) continue;
      const separator = line.search(/\s/);
      if (separator <= 0) {
        setMessage(`第 ${index + 1} 行格式无效`);
        return;
      }
      const account = line.slice(0, separator).trim();
      const password = line.slice(separator).trimStart();
      if (!account || !password) {
        setMessage(`第 ${index + 1} 行格式无效`);
        return;
      }
      accounts.push({ account, password });
    }
    if (accounts.length === 0) {
      setMessage("没有可导入的 Steam 网页账号");
      return;
    }
    setShowSteamTextImport(false);
    setSteamTextImportDraft("");
    const result = await runTask("正在批量导入 Steam 网页账号...", () => {
      const request = invoke<SteamBulkImportResult>("import_steam_web_accounts_from_text", { accounts });
      accounts.forEach((entry) => { entry.password = ""; });
      return request;
    });
    const verificationSummary = result.verificationRequiredAccounts.length > 0
      ? `，需验证已跳过 ${result.verificationRequiredAccounts.length}（${result.verificationRequiredAccounts.join("、")}）`
      : "";
    setMessage(result.failed > 0 || result.verificationRequiredAccounts.length > 0
      ? `Steam 网页账号导入完成：成功 ${result.imported}${verificationSummary}，其他失败 ${result.failed}`
      : `已导入 ${result.imported} 个 Steam 网页账号`);
  }

  async function openSteamWebSession(session: SteamWebSession) {
    await runTask("正在打开 Steam 网页账号...", () => invoke("open_steam_web_session", { sessionId: session.id }));
    setMessage(`已打开 ${session.displayName}`);
  }

  async function refreshSteamWebSessions() {
    const workspace = await runTask("正在识别 Steam 网页账号...", () => invoke<SteamWorkspace>("refresh_steam_web_sessions"));
    setData((current) => ({ ...current, steam: workspace }));
    setMessage("Steam 网页账号已刷新");
  }

  function selectSteamWebSession(session: SteamWebSession) {
    setSelectedSteamWebSessionId(session.id);
    setSteamWebNoteDraft(session.note || "");
  }

  async function saveSteamWebSessionNote(session: SteamWebSession) {
    const workspace = await runTask("正在保存网页账号备注...", () => invoke<SteamWorkspace>("set_steam_web_session_note", { sessionId: session.id, note: steamWebNoteDraft }));
    setData((current) => ({ ...current, steam: workspace }));
    setMessage("Steam 网页账号备注已保存");
  }

  async function deleteSteamWebSession(session: SteamWebSession) {
    await runTask("正在删除 Steam 网页账号...", () => invoke("delete_steam_web_session", { sessionId: session.id }));
    setData((current) => ({ ...current, steam: { ...current.steam, webSessions: current.steam.webSessions.filter((item) => item.id !== session.id) } }));
    setPendingDeleteSteamWebSession(null);
    if (selectedSteamWebSessionId === session.id) setSelectedSteamWebSessionId(null);
    setMessage("Steam 网页账号已删除");
  }

  async function switchPerfectWebAccount(session: SteamWebSession) {
    const result = await runTask("正在通过 Steam 网页认证切换完美账号...", () => invoke<SwitchResult>("switch_perfect_web_account", { sessionId: session.id }));
    await refreshPerfectArena();
    setMessage(result.message);
  }

  async function switchSteamAndPerfectAccount(session: SteamWebSession) {
    const result = await runTask("正在同步切换 Steam 与完美账号...", () => invoke<SwitchResult>("switch_steam_and_perfect_account", { sessionId: session.id }));
    const latest = await invoke<AppData>("get_app_data");
    setData(latest);
    await refreshPerfectArena();
    setMessage(result.message);
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

  function showScrollbarWhileScrolling(event: ReactUIEvent<HTMLElement>) {
    const element = event.currentTarget;
    element.classList.add("is-scrolling");
    const previousTimer = scrollTimersRef.current.get(element);
    if (previousTimer) clearTimeout(previousTimer);
    const timer = setTimeout(() => {
      element.classList.remove("is-scrolling");
      scrollTimersRef.current.delete(element);
    }, 800);
    scrollTimersRef.current.set(element, timer);
  }

  useEffect(() => {
    refresh()
      .then(async () => {
        try {
          await invoke("get_config_health");
        } catch (error) {
          setMessage(errorMessage(error));
          return;
        }
        await validate();
      })
      .catch(() => setMessage("未找到 OOPZ，请在概览里手动选择目录"));
    invoke<UpdateStatus>("get_update_status").then((status) => {
      setUpdateStatus(status);
      if (status.state === "updated" || status.state === "error") setMessage(status.message);
    }).catch(() => undefined);
    refreshPluginStatus().catch(() => undefined);
    refreshPerfectArena().catch(() => undefined);

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
      refreshPerfectArena().catch(() => undefined);
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
    keepListener(listen<string>("steam-web-session-verified", (event) => {
      refresh().catch(() => undefined);
      setMessage(`Steam 网页账号已新增：${event.payload}`);
    }));
    keepListener(listen<string>("steam-web-session-error", (event) => {
      setMessage(event.payload);
    }));
    keepListener(listen<string>("steam-bulk-import-progress", (event) => {
      setMessage(event.payload);
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
    if (activeApp === "oopz" && activeFeature === "switcher" && !scannedOnceRef.current) {
      refreshAccounts(false).catch(() => undefined);
    }
    if (activeApp === "oopz" && activeFeature === "switcher") {
      refreshPluginStatus().catch(() => undefined);
    }
    if (activeApp === "perfect") {
      refreshPerfectArena().catch(() => undefined);
    }
  }, [activeApp, activeFeature]);

  useEffect(() => {
    if (!pendingDeleteAccount && !pendingDeleteSteamAccount && !pendingDeleteSteamWebSession && !showSteamTextImport && !showShareCenter) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !busy) {
        setPendingDeleteAccount(null);
        setPendingDeleteSteamAccount(null);
        setPendingDeleteSteamWebSession(null);
        setShowSteamTextImport(false);
        if (!wormholeActive) setShowShareCenter(false);
      }
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [pendingDeleteAccount, pendingDeleteSteamAccount, pendingDeleteSteamWebSession, showSteamTextImport, showShareCenter, wormholeActive, busy]);

  useEffect(() => () => {
    scrollTimersRef.current.forEach((timer) => clearTimeout(timer));
    scrollTimersRef.current.clear();
  }, []);

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
          <div className="account-list account-list-compact auto-hide-scrollbar" onScroll={showScrollbarWhileScrolling}>
            {data.accounts.length === 0 && <div className="empty">暂无账号。先打开 OOPZ 登录一次，再点刷新。</div>}
            {data.accounts.map((account) => (
              <div className="account-row" data-selected={selected?.id === account.id} key={account.id}>
                <div className="account-row-main">
                  <button className="account-main" onClick={() => setSelectedId(account.id)} aria-expanded={selected?.id === account.id}>
                    <AccountAvatar account={account} ready={account.hasLoginState} />
                    <span><strong>{account.displayName}</strong><small>{accountLabel(account)}</small></span>
                  </button>
                  <div className="account-actions">
                    <button className="icon-button danger" onClick={() => setPendingDeleteAccount(account)} disabled={busy} aria-label={`删除 ${account.displayName} 的登录态`} title="删除登录态"><Trash2 size={16} strokeWidth={2} /></button>
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

  const steamOverview = (
    <section className="content-stack">
      <div className="panel">
        <div className="panel-title"><h2>Steam 状态</h2></div>
        <dl className="paths">
          <dt>程序</dt><dd>{data.steam.installation?.executable || "未设置"}</dd>
          <dt>安装目录</dt><dd>{data.steam.installation?.installDir || "未设置"}</dd>
        </dl>
      </div>
    </section>
  );

  const steamSwitcher = (
    <section className="content-stack">
      <div className="panel">
        <div className="panel-title"><h2>Steam 网页账号</h2><div className="actions"><button onClick={() => handleAction(refreshSteamWebSessions)} disabled={busy || data.steam.webSessions.length === 0}>刷新账号</button><button onClick={() => setShowSteamTextImport(true)} disabled={busy}>从文本导入</button><button className="primary" onClick={() => handleAction(createSteamWebSession)} disabled={busy}>新增</button></div></div>
        <div className="account-list account-list-compact auto-hide-scrollbar" onScroll={showScrollbarWhileScrolling}>
          {data.steam.webSessions.length === 0 && <div className="empty">暂无 Steam 网页账号。</div>}
          {data.steam.webSessions.map((session) => (
            <div className="account-row" data-selected={selectedSteamWebSessionId === session.id} key={session.id}>
              <div className="account-row-main">
                <button className="account-main" onClick={() => selectSteamWebSession(session)} aria-expanded={selectedSteamWebSessionId === session.id}><span className="avatar-wrap"><span className="avatar-fallback">S</span></span><span><strong>{session.accountName || session.displayName}</strong><small>{session.steamId ? `SteamID: ${session.steamId}` : "等待登录识别"}{session.note ? ` · ${session.note}` : ""}</small></span></button>
                <div className="account-actions"><button className="icon-button danger" onClick={() => setPendingDeleteSteamWebSession(session)} disabled={busy} aria-label={`删除 ${session.displayName} 的网页会话`} title="删除网页账号"><Trash2 size={16} /></button><button className={session.steamId ? "primary" : ""} onClick={() => handleAction(() => openSteamWebSession(session))} disabled={busy}>{session.steamId ? "打开" : "登录"}</button></div>
              </div>
              {selectedSteamWebSessionId === session.id && <div className="steam-account-note"><input value={steamWebNoteDraft} onChange={(event) => setSteamWebNoteDraft(event.target.value)} maxLength={120} placeholder="添加账号备注" /><button onClick={() => handleAction(() => saveSteamWebSessionNote(session))} disabled={busy}>保存备注</button></div>}
            </div>
          ))}
        </div>
      </div>
      <div className="panel">
        <div className="panel-title"><h2>Steam 客户端账号</h2><div className="actions"><button onClick={() => handleAction(discoverSteam)} disabled={busy}>搜索并刷新</button></div></div>
        <div className="account-list account-list-compact auto-hide-scrollbar" onScroll={showScrollbarWhileScrolling}>
          {data.steam.accounts.length === 0 && <div className="empty">未发现 Steam 账号，请先关闭 Steam 后点击刷新，或完成一次登录。</div>}
          {data.steam.accounts.map((account) => (
            <div className="account-row" data-selected={data.steam.currentAccountId === account.id || selectedSteamId === account.id} key={account.id}>
              <div className="account-row-main">
                <button className="account-main" onClick={() => selectSteamAccount(account)} aria-expanded={selectedSteamId === account.id}><span className="avatar-wrap"><span className="avatar-fallback">S</span></span><span><strong>{account.displayName}</strong><small>{account.accountName} · {account.id}{account.note ? ` · ${account.note}` : ""}</small></span></button>
                <div className="account-actions"><button className="icon-button danger" onClick={() => setPendingDeleteSteamAccount(account)} disabled={busy} aria-label={`删除 ${account.displayName} 的 NEA 快照`} title="删除账号快照"><Trash2 size={16} /></button><button className={account.mostRecent ? "" : "primary"} onClick={() => handleAction(() => switchSteamAccount(account))} disabled={busy || account.mostRecent}>{account.mostRecent ? "当前登录" : "快速切号"}</button></div>
              </div>
              {selectedSteamId === account.id && <div className="steam-account-note"><input value={steamNoteDraft} onChange={(event) => setSteamNoteDraft(event.target.value)} maxLength={120} placeholder="添加账号备注" /><button onClick={() => handleAction(() => saveSteamNote(account))} disabled={busy}>保存备注</button></div>}
            </div>
          ))}
        </div>
      </div>
    </section>
  );

  const perfectOverview = (
    <section className="content-stack">
      <div className="panel">
        <div className="panel-title"><h2>完美对战平台状态</h2><div className="actions"><button onClick={() => handleAction(discoverPerfectArena)} disabled={busy}>重新搜索</button></div></div>
        <dl className="paths">
          <dt>程序</dt><dd>{perfectWorkspace.installation?.executable || "未找到"}</dd>
          <dt>运行状态</dt><dd>{perfectWorkspace.running ? "运行中" : "未运行"}</dd>
          <dt>当前账号</dt><dd>{data.steam.webSessions.find((session) => session.steamId === perfectWorkspace.currentAccountId)?.displayName || perfectWorkspace.accounts.find((account) => account.id === perfectWorkspace.currentAccountId)?.displayName || "未识别"}</dd>
        </dl>
      </div>
    </section>
  );

  const perfectSwitcher = (
    <section className="perfect-switcher">
      <header className="perfect-switcher-header">
        <h2>账号列表</h2>
        <div className="actions"><button onClick={() => handleAction(refreshPerfectProfiles)} disabled={busy || data.steam.webSessions.every((session) => !session.steamId)}>刷新资料</button><button className="primary" onClick={() => handleAction(createSteamWebSession)} disabled={busy}>新增账号</button></div>
      </header>
      <div className="perfect-filter-bar">
        <input type="search" value={perfectSearch} onChange={(event) => setPerfectSearch(event.target.value)} placeholder="搜索 ID、名称或备注" aria-label="搜索完美账号" />
        <select value={perfectScoreFilter} onChange={(event) => setPerfectScoreFilter(event.target.value)} aria-label="按分数段筛选">
          <option value="all">全部等级分</option>
          <option value="pending">等级分待检测</option>
          <option value="under-1000">1000 以下</option>
          <option value="1000-1499">1000 - 1499</option>
          <option value="1500-1999">1500 - 1999</option>
          <option value="2000-plus">2000 以上</option>
        </select>
        <label><input type="checkbox" checked={perfectPendingOnly} onChange={(event) => setPerfectPendingOnly(event.target.checked)} />仅显示待检测</label>
        <label><input type="checkbox" checked={perfectAvailableOnly} onChange={(event) => setPerfectAvailableOnly(event.target.checked)} />仅显示可用账号</label>
      </div>
      <div className="perfect-account-grid">
        {data.steam.webSessions.length === 0 && <div className="empty perfect-grid-empty">暂无账号。</div>}
        {data.steam.webSessions.length > 0 && filteredPerfectSessions.length === 0 && <div className="empty perfect-grid-empty">没有符合筛选条件的账号。</div>}
        {filteredPerfectSessions.map((session) => {
          const current = Boolean(session.steamId && session.steamId === perfectWorkspace.currentAccountId);
          const profile = session.steamId ? perfectProfiles[session.steamId] : undefined;
          const matchingSteamAccount = session.steamId ? data.steam.accounts.find((account) => account.id === session.steamId) : undefined;
          const selected = selectedSteamWebSessionId === session.id;
          const requiresVerification = Boolean(profile?.highRisk || profile?.reputationRequiresVerification);
          const unavailable = Boolean(session.steamId && data.perfectUnavailableAccountIds?.includes(session.steamId));
          const reputationLabel = unavailable ? "不可用" : requiresVerification ? "高危" : profile?.reputationLevel || "待检测";
          const reputationDetail = unavailable ? null : requiresVerification ? "需验证" : profile?.reputationPoints;
          return (
            <article className="perfect-account-card" data-current={current} data-selected={selected} data-high-risk={requiresVerification || unavailable || undefined} key={session.id}>
              {session.steamId && <div className="perfect-card-menu-wrap">
                <button className="icon-button perfect-card-menu-button" onClick={() => setPerfectMenuSessionId((value) => value === session.id ? null : session.id)} aria-label="账号菜单" title="账号菜单"><MoreHorizontal size={17} /></button>
                {perfectMenuSessionId === session.id && <div className="perfect-card-menu">
                  <button onClick={() => handleAction(() => setPerfectAccountUnavailable(session, !unavailable))}>{unavailable ? "恢复为可用账号" : "标记为不可用账号"}</button>
                </div>}
              </div>}
              <button className="perfect-card-identity" onClick={() => selectSteamWebSession(session)} aria-expanded={selected}>
                <span className="perfect-card-avatar">{profile?.avatarUrl ? <img src={profile.avatarUrl} alt="" referrerPolicy="no-referrer" /> : <span className="avatar-fallback">P</span>}</span>
                <span className="perfect-card-name"><strong>{profile?.nickname || session.accountName || session.displayName}</strong><small>{session.steamId || "等待登录识别"}</small></span>
                {current && <span className="perfect-current-badge">当前</span>}
              </button>
              <div className="perfect-card-metrics">
                <span><small>等级分</small><strong>{perfectScoreLabel(profile?.score)}</strong></span>
                <span><small>玩家身份</small><strong>{profile?.playerIdentity || "待检测"}</strong></span>
                <span className="perfect-reputation" data-level={requiresVerification || unavailable ? "danger" : profile?.reputationLevel || "pending"}><small>信誉等级</small><strong>{reputationLabel}{reputationDetail != null ? <em>{reputationDetail}</em> : null}</strong></span>
              </div>
              {selected && <div className="steam-account-note"><input value={steamWebNoteDraft} onChange={(event) => setSteamWebNoteDraft(event.target.value)} maxLength={120} placeholder="添加账号备注" /><button onClick={() => handleAction(() => saveSteamWebSessionNote(session))} disabled={busy}>保存</button></div>}
              <div className="perfect-card-actions">
                <button className="icon-button danger" onClick={() => setPendingDeleteSteamWebSession(session)} disabled={busy} aria-label={`删除 ${session.displayName}`} title="删除账号"><Trash2 size={16} /></button>
                <button className={matchingSteamAccount ? "primary" : ""} onClick={() => handleAction(() => switchSteamAndPerfectAccount(session))} disabled={busy || !matchingSteamAccount || !perfectWorkspace.installation || !data.steam.installation} title={matchingSteamAccount ? "同步切换 Steam 客户端与完美账号" : "未找到相同 SteamID 的 Steam 客户端账号"}>同步切号</button>
                {session.steamId
                  ? <button className={current ? "" : "primary"} onClick={() => handleAction(() => switchPerfectWebAccount(session))} disabled={busy || current || !perfectWorkspace.installation}>{current ? "当前登录" : "快速切换"}</button>
                  : <button onClick={() => handleAction(() => openSteamWebSession(session))} disabled={busy}>登录</button>}
              </div>
            </article>
          );
        })}
      </div>
    </section>
  );

  function selectApp(app: AppKey) {
    setActiveApp(app);
    setActiveFeature("switcher");
  }

  const activeContent = activeApp === "oopz"
    ? activeFeature === "overview" ? overview : switcher
    : activeApp === "steam"
      ? activeFeature === "overview" ? steamOverview : steamSwitcher
      : activeFeature === "overview" ? perfectOverview : perfectSwitcher;
  const activeAppName = activeApp === "oopz" ? "OOPZ" : activeApp === "steam" ? "Steam" : "完美对战平台";

  return (
    <main className="shell">
      <header className="window-titlebar" data-tauri-drag-region onMouseDown={startWindowDrag} onDoubleClick={toggleMaximizeWindow}>
        <div className="window-brand" data-tauri-drag-region>
          <img src="/nea-brand-dark.png" alt="NEA - Not Enough Accounts" draggable={false} data-tauri-drag-region />
        </div>
        <div className="window-controls">
          <button className="window-update" onClick={() => handleAction(checkForUpdates)} disabled={busy || updateActive} aria-label="检查更新" title={updateStatus?.message || "检查更新"}><RefreshCw className={updateActive ? "spin-icon" : ""} size={15} /></button>
          <button onClick={minimizeWindow} onDoubleClick={(event) => event.stopPropagation()} aria-label="最小化" title="最小化"><Minus size={15} /></button>
          <button onClick={toggleMaximizeWindow} onDoubleClick={(event) => event.stopPropagation()} aria-label="最大化或还原" title="最大化或还原"><Square size={13} /></button>
          <button className="window-close" onClick={closeWindow} onDoubleClick={(event) => event.stopPropagation()} aria-label="关闭" title="关闭"><X size={16} /></button>
        </div>
      </header>

      <div className="app-layout">
        <aside className="app-rail">
          <nav className="app-list" aria-label="软件切换">
            <button data-active={activeApp === "oopz"} onClick={() => selectApp("oopz")} aria-label="切换到 OOPZ" title="OOPZ"><img className="app-icon-image" src="/oopz-icon.png" alt="" /></button>
            <button data-active={activeApp === "steam"} onClick={() => selectApp("steam")} aria-label="切换到 Steam" title="Steam"><img className="app-icon-image" src="/steam-icon.svg" alt="" /></button>
            <button data-active={activeApp === "perfect"} onClick={() => selectApp("perfect")} aria-label="切换到完美对战平台" title="完美对战平台"><img className="app-icon-image" src="/perfect-arena-icon.png" alt="" /></button>
          </nav>
          <button className="global-share-button" onClick={openShareCenter} aria-label="账号分享" title="账号分享"><Share2 size={20} strokeWidth={1.9} /></button>
        </aside>
        <aside className="sidebar auto-hide-scrollbar" onScroll={showScrollbarWhileScrolling}>
          <div className="sidebar-app-name">{activeAppName}</div>
          <nav className="feature-list">
            <button data-active={activeFeature === "overview"} onClick={() => setActiveFeature("overview")}><LayoutDashboard size={17} strokeWidth={2} aria-hidden="true" /><strong>概览</strong></button>
            <button data-active={activeFeature === "switcher"} onClick={() => setActiveFeature("switcher")}><UsersRound size={17} strokeWidth={2} aria-hidden="true" /><strong>账号切换</strong></button>
          </nav>
        </aside>

        <section className="workspace auto-hide-scrollbar" onScroll={showScrollbarWhileScrolling}>
          <header className="topbar">
            <h2>{activeAppName} · {activeFeature === "overview" ? "概览" : "账号切换"}</h2>
            <div className="status" data-busy={busy}>{busy && <span className="spinner" />}<span>{message}</span></div>
          </header>
          {activeContent}
        </section>
      </div>
      {showShareCenter && (
        <div className="confirm-backdrop share-center-backdrop" onMouseDown={() => !wormholeActive && setShowShareCenter(false)}>
          <div className="share-center" role="dialog" aria-modal="true" aria-labelledby="share-center-title" onMouseDown={(event) => event.stopPropagation()}>
            <header className="share-center-header">
              <div><h2 id="share-center-title">账号分享</h2><p>Magic Wormhole 端到端加密传输，选择要发送的账号。</p></div>
              <button className="icon-button" onClick={() => setShowShareCenter(false)} disabled={wormholeActive} aria-label="关闭"><X size={17} /></button>
            </header>
            <div className="share-center-toolbar">
              <button onClick={selectAllShareableAccounts} disabled={wormholeActive}>全选可分享账号</button>
              <button onClick={() => setShareSelection({ oopzAccountIds: [], steamWebSessionIds: [], perfectSessionIds: [] })} disabled={wormholeActive}>清空</button>
              <span>已选择 {selectedShareCount} 个账号</span>
            </div>
            <div className="share-tree auto-hide-scrollbar" onScroll={showScrollbarWhileScrolling}>
              <section className="share-tree-platform">
                <label className="share-tree-parent"><input type="checkbox" checked={shareableOopzAccounts.length > 0 && shareSelection.oopzAccountIds.length === shareableOopzAccounts.length} onChange={(event) => toggleShareBranch("oopzAccountIds", shareableOopzAccounts.map((account) => account.id), event.target.checked)} disabled={wormholeActive || shareableOopzAccounts.length === 0} /><img src="/oopz-icon.png" alt="" /><strong>OOPZ</strong><span>{shareableOopzAccounts.length} 个可分享</span></label>
                <div className="share-tree-children">
                  {shareableOopzAccounts.length === 0 && <div className="share-tree-empty">暂无可分享登录态</div>}
                  {shareableOopzAccounts.map((account) => <label key={account.id}><input type="checkbox" checked={shareSelection.oopzAccountIds.includes(account.id)} onChange={(event) => toggleShareItem("oopzAccountIds", account.id, event.target.checked)} disabled={wormholeActive} /><span><strong>{account.displayName}</strong><small>{accountLabel(account)}</small></span></label>)}
                </div>
              </section>
              <section className="share-tree-platform" data-disabled="true">
                <label className="share-tree-parent"><input type="checkbox" disabled /><img src="/steam-icon.svg" alt="" /><strong>Steam 客户端</strong><span className="unsupported-badge">暂不支持</span></label>
                <div className="share-tree-children">
                  {data.steam.accounts.length === 0 && <div className="share-tree-empty">暂无账号</div>}
                  {data.steam.accounts.map((account) => <label key={account.id}><input type="checkbox" disabled /><span><strong>{account.note || account.displayName}</strong><small>{account.accountName}</small></span><em>暂不支持</em></label>)}
                </div>
              </section>
              <section className="share-tree-platform">
                <label className="share-tree-parent"><input type="checkbox" checked={shareableWebSessions.length > 0 && shareSelection.steamWebSessionIds.length === shareableWebSessions.length} onChange={(event) => toggleShareBranch("steamWebSessionIds", shareableWebSessions.map((session) => session.id), event.target.checked)} disabled={wormholeActive || shareableWebSessions.length === 0} /><img src="/steam-icon.svg" alt="" /><strong>Steam 网页账号</strong><span>支持跨机器</span></label>
                <div className="share-tree-children">
                  {shareableWebSessions.map((session) => <label key={session.id}><input type="checkbox" checked={shareSelection.steamWebSessionIds.includes(session.id)} onChange={(event) => toggleShareItem("steamWebSessionIds", session.id, event.target.checked)} disabled={wormholeActive} /><span><strong>{session.note || session.displayName}</strong><small>{session.steamId}</small></span></label>)}
                </div>
              </section>
              <section className="share-tree-platform">
                <div className="share-tree-parent share-tree-perfect-parent">
                  <label className="share-tree-parent-select"><input type="checkbox" checked={selectablePerfectSessions.length > 0 && selectablePerfectSessions.every((session) => shareSelection.perfectSessionIds.includes(session.id))} onChange={(event) => toggleShareBranch("perfectSessionIds", selectablePerfectSessions.map((session) => session.id), event.target.checked)} disabled={wormholeActive || selectablePerfectSessions.length === 0} /><img src="/perfect-arena-icon.png" alt="" /><strong>完美对战平台</strong></label>
                  <label className="share-available-filter"><input type="checkbox" checked={sharePerfectAvailableOnly} onChange={(event) => setPerfectShareAvailableOnly(event.target.checked)} disabled={wormholeActive} />仅选择可用账号</label>
                </div>
                <div className="share-tree-children">
                  {shareableWebSessions.map((session) => {
                    const profile = perfectProfiles[session.steamId || ""];
                    const unavailable = Boolean(session.steamId && data.perfectUnavailableAccountIds?.includes(session.steamId));
                    const highRisk = Boolean(profile?.highRisk || profile?.reputationRequiresVerification);
                    const reputation = unavailable ? "不可用" : highRisk ? "高危" : profile?.reputationLevel || "待检测";
                    const disabledByFilter = sharePerfectAvailableOnly && !isPerfectShareUsable(session);
                    return <label key={session.id} data-unavailable={unavailable || highRisk || undefined}><input type="checkbox" checked={shareSelection.perfectSessionIds.includes(session.id)} onChange={(event) => toggleShareItem("perfectSessionIds", session.id, event.target.checked)} disabled={wormholeActive || disabledByFilter} /><span><strong>{profile?.nickname || session.note || session.displayName}</strong><small>{session.steamId}</small><span className="share-perfect-meta"><b>等级分 {perfectScoreLabel(profile?.score)}</b><b>身份 {profile?.playerIdentity || "待检测"}</b><b data-danger={unavailable || highRisk || undefined}>信誉 {reputation}</b></span></span></label>;
                  })}
                </div>
              </section>
            </div>
            <div className="share-center-transfer">
              <div className="quick-transfer-row">
                <button className="primary" onClick={() => void startQuickShare()} disabled={busy || wormholeActive || selectedShareCount === 0}>生成分享码</button>
                {quickCode && <code className="quick-code">{quickCode}</code>}
                {quickCode && <button onClick={() => copyText(quickCode)}>复制代码</button>}
                {wormholeActive && <button onClick={() => void cancelQuickShare()} disabled={wormholeStatus?.state === "cancelling"}>取消</button>}
              </div>
              <div className="quick-transfer-row">
                <input value={receiveCode} onChange={(event) => setReceiveCode(event.target.value)} placeholder="输入对方分享码" disabled={busy || wormholeActive} />
                <button onClick={() => handleAction(quickImport)} disabled={busy || wormholeActive || !receiveCode.trim()}>接收并导入</button>
              </div>
              {wormholeStatus && <div className="quick-transfer-status" data-state={wormholeStatus.state}>{wormholeStatus.message}</div>}
              {wormholeStatus?.total && wormholeStatus.transferred !== undefined && <progress value={wormholeStatus.transferred} max={wormholeStatus.total} />}
              <p className="share-dedupe-note">同一账号同时勾选 Steam 网页与完美平台时，只发送完美平台项，避免重复。</p>
            </div>
          </div>
        </div>
      )}
      {pendingDeleteAccount && (
        <div className="confirm-backdrop" onMouseDown={() => !busy && setPendingDeleteAccount(null)}>
          <div className="confirm-dialog" role="alertdialog" aria-modal="true" aria-labelledby="delete-confirm-title" onMouseDown={(event) => event.stopPropagation()}>
            <p id="delete-confirm-title">确定删除账号“{pendingDeleteAccount.displayName}”吗？</p>
            <div className="confirm-actions">
              <button className="primary danger-confirm" onClick={confirmDeleteSelected} disabled={busy} autoFocus>确定</button>
              <button onClick={() => setPendingDeleteAccount(null)} disabled={busy}>取消</button>
            </div>
          </div>
        </div>
      )}
      {pendingDeleteSteamAccount && (
        <div className="confirm-backdrop" onMouseDown={() => !busy && setPendingDeleteSteamAccount(null)}>
          <div className="confirm-dialog" role="alertdialog" aria-modal="true" aria-labelledby="steam-delete-confirm-title" onMouseDown={(event) => event.stopPropagation()}>
            <p id="steam-delete-confirm-title">确定删除账号“{pendingDeleteSteamAccount.displayName}”吗？</p>
            <div className="confirm-actions"><button className="primary danger-confirm" onClick={() => handleAction(() => deleteSteamAccount(pendingDeleteSteamAccount))} disabled={busy} autoFocus>确定</button><button onClick={() => setPendingDeleteSteamAccount(null)} disabled={busy}>取消</button></div>
          </div>
        </div>
      )}
      {pendingDeleteSteamWebSession && (
        <div className="confirm-backdrop" onMouseDown={() => !busy && setPendingDeleteSteamWebSession(null)}>
          <div className="confirm-dialog" role="alertdialog" aria-modal="true" aria-labelledby="steam-web-delete-confirm-title" onMouseDown={(event) => event.stopPropagation()}>
            <p id="steam-web-delete-confirm-title">确定删除网页账号“{pendingDeleteSteamWebSession.displayName}”吗？</p>
            <div className="confirm-actions"><button className="primary danger-confirm" onClick={() => handleAction(() => deleteSteamWebSession(pendingDeleteSteamWebSession))} disabled={busy} autoFocus>确定</button><button onClick={() => setPendingDeleteSteamWebSession(null)} disabled={busy}>取消</button></div>
          </div>
        </div>
      )}
      {showSteamTextImport && (
        <div className="confirm-backdrop" onMouseDown={() => !busy && setShowSteamTextImport(false)}>
          <div className="confirm-dialog steam-text-import-dialog" role="dialog" aria-modal="true" aria-labelledby="steam-text-import-title" onMouseDown={(event) => event.stopPropagation()}>
            <p id="steam-text-import-title">从文本导入 Steam 网页账号</p>
            <textarea value={steamTextImportDraft} onChange={(event) => setSteamTextImportDraft(event.target.value)} placeholder={"账号 密码\n账号 密码"} autoFocus spellCheck={false} />
            <div className="confirm-actions"><button className="primary" onClick={() => handleAction(importSteamWebAccountsFromText)} disabled={busy || !steamTextImportDraft.trim()}>导入</button><button onClick={() => setShowSteamTextImport(false)} disabled={busy}>取消</button></div>
          </div>
        </div>
      )}
    </main>
  );
}

export default App;

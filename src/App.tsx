import { useEffect, useMemo, useRef, useState, type MouseEvent, type UIEvent as ReactUIEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open, save } from "@tauri-apps/plugin-dialog";
import { CalendarCheck, Eye, EyeOff, KeyRound, LayoutDashboard, Minus, Moon, MoreHorizontal, RefreshCw, Share2, Square, Sun, Trash2, UsersRound, X } from "lucide-react";
import "./App.css";

type AppConfig = {
  oopzInstallDir?: string;
  oopzExePath?: string;
  roamingDataDir?: string;
  localSandboxDir?: string;
  pluginModeEnabled?: boolean;
  pluginAutostartEnabled?: boolean;
  oopzAutoSignEnabled?: boolean;
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
  steamCredentials?: SteamSavedCredential[];
  steamIdentities?: SteamIdentity[];
  perfectUnavailableAccountIds?: string[];
  currentLoginUid?: string;
};

type SteamSavedCredential = {
  accountName: string;
  password: string;
  steamId?: string;
  updatedAt: string;
};

type SteamIdentity = {
  id: string;
  steamId?: string;
  accountName?: string;
  displayName: string;
  note?: string;
  clientAccountId?: string;
  webSessionId?: string;
  capabilities: {
    webLogin: boolean;
    credential: boolean;
    perfectProfile: boolean;
  };
  createdAt: string;
  updatedAt: string;
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
  clientOnline: boolean;
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
  cancelled: number;
  skippedExisting: number;
  skippedExistingAccounts: string[];
  skippedDuplicateInput: number;
  invalidCredentialAccounts: string[];
  tokenProtectedAccounts: string[];
  verificationRequiredAccounts: string[];
  failedAccounts: string[];
  cancelledAccounts: string[];
};

type SteamImportPreview = {
  existingAccounts: string[];
  duplicateInputAccounts: string[];
};

type SteamCapabilityCompletionResult = {
  checked: number;
  processed: number;
  alreadyComplete: number;
  webCompleted: number;
  cancelled: boolean;
  verificationRequiredAccounts: string[];
  failedAccounts: string[];
};

type SteamCapabilityStatus = {
  running: boolean;
  paused: boolean;
  cancelling: boolean;
};

type PendingOopzOperation =
  | { kind: "switch"; account: SavedAccount }
  | { kind: "restore" };

type StorageOptimizationResult = {
  beforeBytes: number;
  afterBytes: number;
  freedBytes: number;
  optimizedSessions: number;
  removedOrphanSessions: number;
  cachedPerfectAvatars: number;
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
  state: "preparing" | "waiting" | "connecting" | "transferring" | "importing" | "committing" | "cancelling" | "cancelled" | "complete" | "error";
  direction: "send" | "receive";
  message: string;
  code?: string;
  transferred?: number;
  total?: number;
  packageBytes?: number;
};

type QuickShareSelection = {
  oopzAccountIds: string[];
  steamAccounts: SteamShareChoice[];
};

type SteamShareChoice = {
  steamId: string;
  webLogin: boolean;
  credential: boolean;
  perfect: boolean;
  credentialFromPerfect?: boolean;
};

type QuickImportResult = {
  oopzAccounts: SavedAccount[];
  steamWebAccounts: number;
  perfectAccounts: number;
  steamWebAdded: number;
  steamWebUpdated: number;
  perfectAdded: number;
  perfectUpdated: number;
  steamCredentialsAccounts: number;
  steamCredentialsAdded: number;
};

type QuickShareExportResult = {
  accounts: number;
  packageBytes: number;
};

type OopzAutoSignStatus = {
  enabled: boolean;
  state: "disabled" | "waiting" | "checking" | "signed" | "error";
  message: string;
  accountUid?: string;
  signedToday: boolean;
  accumulatedDays?: number;
  freeCoinBalance?: number;
  rewardName?: string;
  rewardQuantity?: number;
  lastCheckedAt?: string;
  lastSignedAt?: string;
};

const MAX_SHARED_PLATFORM_ACCOUNTS = 100;

type AppKey = "oopz" | "steam" | "perfect";
type FeatureKey = "overview" | "switcher" | "autoSign";
type PerfectAvailability = "ready" | "pending" | "blocked";
type StartupViewPhase = "loading" | "ready" | "error";

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

function perfectRank(score?: number) {
  if (score == null) return undefined;
  return score <= 1000 ? "D"
    : score <= 1150 ? "C"
      : score <= 1300 ? "C+"
        : score <= 1450 ? "金C+"
          : score <= 1600 ? "B"
            : score <= 1750 ? "B+"
              : score <= 1900 ? "金B+"
                : score <= 2050 ? "A"
                  : score <= 2200 ? "A+"
                    : "金A+";
}

function perfectScoreLabel(score?: number) {
  const rank = perfectRank(score);
  return rank && score != null ? `${rank}${score}` : "待检测";
}

function perfectAvailability(profile?: PerfectArenaProfile, manuallyUnavailable = false): PerfectAvailability {
  if (manuallyUnavailable || profile?.highRisk || profile?.reputationRequiresVerification) return "blocked";
  if (!profile?.found || profile.score == null || !profile.playerIdentity || !profile.reputationLevel) return "pending";
  return "ready";
}

function formatFileSize(bytes: number) {
  if (!Number.isFinite(bytes) || bytes < 0) return "未知大小";
  if (bytes < 1024) return `${Math.round(bytes)} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }
  const digits = value >= 100 ? 0 : value >= 10 ? 1 : 2;
  return `${value.toFixed(digits)} ${units[unitIndex]}`;
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

function readPreference(key: string) {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writePreference(key: string, value: string) {
  try {
    window.localStorage.setItem(key, value);
  } catch {
    // Preferences are optional; a restricted WebView storage policy must not block the app.
  }
}

function App() {
  const [darkMode, setDarkMode] = useState(() => {
    const saved = readPreference("nea-theme");
    if (saved === "dark") return true;
    if (saved === "light") return false;
    return window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? false;
  });
  const [data, setData] = useState<AppData>({ config: {}, accounts: [], steam: { accounts: [], clientOnline: false, webSessions: [] } });
  const [paths, setPaths] = useState<OopzPaths | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [message, setMessage] = useState("正在初始化...");
  const [startupPhase, setStartupPhase] = useState<StartupViewPhase>("loading");
  const [startupError, setStartupError] = useState("");
  const [busy, setBusy] = useState(false);
  const [searchingOopz, setSearchingOopz] = useState(false);
  const [searchPath, setSearchPath] = useState("");
  const [activeApp, setActiveApp] = useState<AppKey>(() => {
    const saved = readPreference("nea-active-app");
    return saved === "steam" || saved === "perfect" ? saved : "oopz";
  });
  const [activeFeature, setActiveFeature] = useState<FeatureKey>(() => {
    const saved = readPreference("nea-active-feature");
    return saved === "switcher" || saved === "autoSign" ? saved : "overview";
  });
  const [pluginStatus, setPluginStatus] = useState<PluginStatus | null>(null);
  const [oopzAutoSignStatus, setOopzAutoSignStatus] = useState<OopzAutoSignStatus | null>(null);
  const [updateStatus, setUpdateStatus] = useState<UpdateStatus | null>(null);
  const [wormholeStatus, setWormholeStatus] = useState<WormholeStatus | null>(null);
  const [quickCode, setQuickCode] = useState("");
  const [quickPackageBytes, setQuickPackageBytes] = useState<number | null>(null);
  const [receiveCode, setReceiveCode] = useState("");
  const [pendingOopzOperation, setPendingOopzOperation] = useState<PendingOopzOperation | null>(null);
  const [pendingDeleteAccount, setPendingDeleteAccount] = useState<SavedAccount | null>(null);
  const [pendingDeleteSteamWebSession, setPendingDeleteSteamWebSession] = useState<SteamWebSession | null>(null);
  const [pendingDeleteSteamCredential, setPendingDeleteSteamCredential] = useState<SteamIdentity | null>(null);
  const [showSteamTextImport, setShowSteamTextImport] = useState(false);
  const [steamTextImportDraft, setSteamTextImportDraft] = useState("");
  const [visibleSteamCredentials, setVisibleSteamCredentials] = useState<string[]>([]);
  const [selectedSteamId, setSelectedSteamId] = useState<string | null>(null);
  const [steamNoteDraft, setSteamNoteDraft] = useState("");
  const [steamSearch, setSteamSearch] = useState("");
  const [steamWebFilter, setSteamWebFilter] = useState("all");
  const [steamClientFilter, setSteamClientFilter] = useState("all");
  const [steamImportPreview, setSteamImportPreview] = useState<SteamImportPreview | null>(null);
  const [steamImportError, setSteamImportError] = useState("");
  const [steamImportResult, setSteamImportResult] = useState<SteamBulkImportResult | null>(null);
  const [steamBulkImportRunning, setSteamBulkImportRunning] = useState(false);
  const [steamBulkImportCancelling, setSteamBulkImportCancelling] = useState(false);
  const [steamCapabilityStatus, setSteamCapabilityStatus] = useState<SteamCapabilityStatus>({ running: false, paused: false, cancelling: false });
  const [steamCapabilityResult, setSteamCapabilityResult] = useState<SteamCapabilityCompletionResult | null>(null);
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
  const [shareSelectionNotice, setShareSelectionNotice] = useState("");
  const [shareSelection, setShareSelection] = useState<QuickShareSelection>({
    oopzAccountIds: [],
    steamAccounts: [],
  });
  const scannedOnceRef = useRef(false);
  const dataSignatureRef = useRef("");
  const busyRef = useRef(false);
  const scrollTimersRef = useRef(new Map<HTMLElement, ReturnType<typeof setTimeout>>());
  const lastPageFocusRef = useRef<HTMLElement | null>(null);
  const activeDialogKey = pendingOopzOperation
    ? pendingOopzOperation.kind === "switch" ? `oopz-switch:${pendingOopzOperation.account.id}` : "oopz-restore"
    : pendingDeleteAccount
      ? `oopz-delete:${pendingDeleteAccount.id}`
      : pendingDeleteSteamWebSession
      ? `steam-web-delete:${pendingDeleteSteamWebSession.id}`
      : pendingDeleteSteamCredential
        ? `steam-credential-delete:${pendingDeleteSteamCredential.id}`
        : showSteamTextImport
          ? "steam-text-import"
          : showShareCenter
            ? "share-center"
            : null;

  const selected = useMemo(
    () => data.accounts.find((account) => account.id === selectedId) || data.accounts[0],
    [data.accounts, selectedId],
  );

  const sessionCount = data.accounts.filter((account) => account.hasLoginState).length;
  const updateActive = updateStatus?.state === "checking" || updateStatus?.state === "downloading" || updateStatus?.state === "installing";
  const wormholeActive = Boolean(wormholeStatus && !["cancelled", "complete", "error"].includes(wormholeStatus.state));
  const shareableOopzAccounts = data.accounts.filter((account) => account.hasLoginState);
  const shareableSteamIdentities = (data.steamIdentities || []).filter((identity) =>
    Boolean(identity.steamId && (identity.capabilities.webLogin || identity.capabilities.credential)),
  );
  const perfectShareIdentities = shareableSteamIdentities.filter((identity) => identity.capabilities.webLogin);
  const isPerfectShareUsable = (identity: SteamIdentity) => {
    if (!identity.steamId) return false;
    const profile = perfectProfiles[identity.steamId];
    return perfectAvailability(profile, data.perfectUnavailableAccountIds?.includes(identity.steamId)) === "ready";
  };
  const isPerfectShareManuallyExcluded = (identity: SteamIdentity) => Boolean(
    identity.steamId && data.perfectUnavailableAccountIds?.includes(identity.steamId),
  );
  const selectablePerfectIdentities = perfectShareIdentities.filter((identity) =>
    !isPerfectShareManuallyExcluded(identity) && (!sharePerfectAvailableOnly || isPerfectShareUsable(identity)),
  );
  const selectablePerfectSteamIds = selectablePerfectIdentities.flatMap((identity) => identity.steamId ? [identity.steamId] : []);
  const shareableOopzIds = shareableOopzAccounts.map((account) => account.id);
  const selectedShareCount = shareSelection.oopzAccountIds.length + shareSelection.steamAccounts.length;
  const filteredPerfectSessions = useMemo(() => {
    const query = perfectSearch.trim().toLocaleLowerCase();
    return data.steam.webSessions.filter((session) => {
      const profile = session.steamId ? perfectProfiles[session.steamId] : undefined;
      const unavailable = Boolean(session.steamId && data.perfectUnavailableAccountIds?.includes(session.steamId));
      const availability = perfectAvailability(profile, unavailable);
      const pending = availability === "pending";
      const score = profile?.score;
      const searchable = [session.steamId, session.accountName, session.displayName, session.note, profile?.nickname]
        .filter(Boolean)
        .join(" ")
        .toLocaleLowerCase();
      if (query && !searchable.includes(query)) return false;
      if (perfectPendingOnly && !pending) return false;
      if (perfectAvailableOnly && availability !== "ready") return false;
      if (perfectScoreFilter === "pending" && score != null) return false;
      if (perfectScoreFilter !== "all" && perfectScoreFilter !== "pending" && perfectRank(score) !== perfectScoreFilter) return false;
      return true;
    });
  }, [data.perfectUnavailableAccountIds, data.steam.webSessions, perfectAvailableOnly, perfectPendingOnly, perfectProfiles, perfectScoreFilter, perfectSearch]);
  const filteredSteamIdentities = useMemo(() => {
    const query = steamSearch.trim().toLocaleLowerCase();
    return (data.steamIdentities || []).filter((identity) => {
      const methods = identity.capabilities;
      if (!methods.webLogin && !methods.credential) return false;
      if (steamWebFilter !== "all" && methods.webLogin !== (steamWebFilter === "yes")) return false;
      if (steamClientFilter !== "all" && methods.credential !== (steamClientFilter === "yes")) return false;
      if (!query) return true;
      const perfectName = identity.steamId ? perfectProfiles[identity.steamId]?.nickname : undefined;
      return [identity.displayName, identity.steamId, identity.accountName, identity.note, perfectName]
        .filter(Boolean)
        .join(" ")
        .toLocaleLowerCase()
        .includes(query);
    });
  }, [data.steamIdentities, perfectProfiles, steamClientFilter, steamSearch, steamWebFilter]);
  const steamIdentityCount = (data.steamIdentities || []).filter((identity) =>
    identity.capabilities.webLogin || identity.capabilities.credential,
  ).length;
  const pendingSteamWebSessions = data.steam.webSessions.filter((session) => !session.steamId);
  const steamFiltersActive = Boolean(steamSearch.trim() || steamWebFilter !== "all" || steamClientFilter !== "all");
  const perfectFiltersActive = Boolean(perfectSearch.trim() || perfectScoreFilter !== "all" || perfectPendingOnly || perfectAvailableOnly);
  const steamImportOtherFailed = steamImportResult?.failedAccounts.length ?? 0;
  const steamImportAttentionAccounts = steamImportResult
    ? Array.from(new Set([
      ...steamImportResult.invalidCredentialAccounts,
      ...steamImportResult.tokenProtectedAccounts,
      ...steamImportResult.verificationRequiredAccounts,
      ...steamImportResult.failedAccounts,
      ...steamImportResult.cancelledAccounts,
    ]))
    : [];

  function resetSteamFilters() {
    setSteamSearch("");
    setSteamWebFilter("all");
    setSteamClientFilter("all");
  }

  function resetPerfectFilters() {
    setPerfectSearch("");
    setPerfectScoreFilter("all");
    setPerfectPendingOnly(false);
    setPerfectAvailableOnly(false);
  }

  function findSteamIdentity(steamId?: string, accountName?: string, webSessionId?: string, clientAccountId?: string) {
    const normalizedAccount = accountName?.trim().toLocaleLowerCase();
    return data.steamIdentities?.find((identity) =>
      Boolean(steamId && identity.steamId === steamId)
      || Boolean(webSessionId && identity.webSessionId === webSessionId)
      || Boolean(clientAccountId && identity.clientAccountId === clientAccountId)
      || Boolean(normalizedAccount && identity.accountName?.trim().toLocaleLowerCase() === normalizedAccount),
    );
  }

  function findSteamCredential(steamId?: string, accountName?: string, webSessionId?: string, clientAccountId?: string) {
    const identity = findSteamIdentity(steamId, accountName, webSessionId, clientAccountId);
    const resolvedSteamId = identity?.steamId || steamId;
    const resolvedAccount = identity?.accountName || accountName;
    const normalizedAccount = resolvedAccount?.trim().toLocaleLowerCase();
    return data.steamCredentials?.find((credential) =>
      Boolean(resolvedSteamId && credential.steamId === resolvedSteamId)
      || Boolean(normalizedAccount && credential.accountName.trim().toLocaleLowerCase() === normalizedAccount),
    );
  }

  function identityCapabilityBadges(identity?: SteamIdentity) {
    if (!identity) return null;
    if (!identity.capabilities.credential) return null;
    return <span className="identity-capabilities" aria-label="可用登录方式">
      <b>账密登录客户端</b>
    </span>;
  }

  function steamLoginMethods(identity: SteamIdentity) {
    return <span className="steam-capability-boundary" aria-label="可用登录方式">
      <b data-available={identity.capabilities.webLogin || undefined}>网页登录 {identity.capabilities.webLogin ? "可直接打开" : "需要登录"}</b>
      <b data-available={identity.capabilities.credential || undefined}>Steam 客户端 {identity.capabilities.credential ? "可账密登录" : "缺少账密"}</b>
    </span>;
  }

  function steamCredentialKey(credential: SteamSavedCredential) {
    return credential.accountName.trim().toLocaleLowerCase();
  }

  function toggleSteamCredential(credential: SteamSavedCredential) {
    const key = steamCredentialKey(credential);
    setVisibleSteamCredentials((current) => current.includes(key) ? [] : [key]);
  }

  function credentialVisible(credential: SteamSavedCredential) {
    return visibleSteamCredentials.includes(steamCredentialKey(credential));
  }

  function credentialEye(credential: SteamSavedCredential, label: string) {
    const visible = credentialVisible(credential);
    return <button className="icon-button credential-eye" type="button" onClick={() => toggleSteamCredential(credential)} aria-label={`${visible ? "隐藏" : "查看"}${label}账号密码`} title={visible ? "隐藏账号密码" : "查看账号密码"}>{visible ? <EyeOff size={16} /> : <Eye size={16} />}</button>;
  }

  function credentialDetails(credential: SteamSavedCredential) {
    if (!credentialVisible(credential)) return null;
    return <div className="steam-credential-details"><span><small>账号</small><code>{credential.accountName}</code></span><span><small>密码</small><code>{credential.password}</code></span></div>;
  }

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

  async function runTask<T>(label: string, task: () => Promise<T>, refreshAfter = true) {
    if (busyRef.current) throw new Error("已有操作正在进行，请稍候");
    busyRef.current = true;
    setBusy(true);
    setMessage(label);
    await new Promise((resolve) => requestAnimationFrame(() => resolve(undefined)));
    try {
      const result = await task();
      if (refreshAfter) await refresh();
      return result;
    } catch (error) {
      if (refreshAfter) await refresh().catch(() => undefined);
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
        { name: "NEA OOPZ 登录态包", extensions: ["oopz+"] },
        { name: "旧版 OOPZ 登录数据", extensions: ["json", "txt"] },
      ],
    });
    if (!source || Array.isArray(source)) return;
    const accounts = await runTask("正在导入账号...", () =>
      invoke<SavedAccount[]>("import_account_package", { path: source }),
    );
    setSelectedId(accounts[0]?.id ?? null);
    setMessage(`已导入 ${accounts.length} 个账号，可快速切换`);
  }

  function rememberDialogOpener(event?: MouseEvent<HTMLButtonElement>) {
    if (event?.currentTarget) lastPageFocusRef.current = event.currentTarget;
  }

  function openShareCenter(event?: MouseEvent<HTMLButtonElement>) {
    rememberDialogOpener(event);
    setSharePerfectAvailableOnly(false);
    setShareSelection({
      oopzAccountIds: [],
      steamAccounts: [],
    });
    setQuickCode("");
    setQuickPackageBytes(null);
    setReceiveCode("");
    setWormholeStatus(null);
    setShareSelectionNotice("");
    setShowShareCenter(true);
  }

  function toggleOopzShareItem(id: string, checked: boolean) {
    setShareSelection((current) => {
      if (checked && !current.oopzAccountIds.includes(id) && current.oopzAccountIds.length >= MAX_SHARED_PLATFORM_ACCOUNTS) {
        setShareSelectionNotice(`OOPZ 一次最多选择 ${MAX_SHARED_PLATFORM_ACCOUNTS} 个账号`);
        return current;
      }
      setShareSelectionNotice("");
      return {
        ...current,
        oopzAccountIds: checked
          ? Array.from(new Set([...current.oopzAccountIds, id]))
          : current.oopzAccountIds.filter((value) => value !== id),
      };
    });
  }

  function oopzShareSelectionState(ids: string[]) {
    const selected = new Set(shareSelection.oopzAccountIds);
    const selectedCount = ids.reduce((count, id) => count + Number(selected.has(id)), 0);
    return {
      any: selectedCount > 0,
      all: ids.length > 0 && selectedCount === ids.length,
    };
  }

  function oopzShareCheckboxRef(element: HTMLInputElement | null, ids: string[]) {
    if (!element) return;
    const state = oopzShareSelectionState(ids);
    element.indeterminate = state.any && !state.all;
  }

  function toggleOopzShareBranch(ids: string[]) {
    setShareSelection((current) => {
      const all = ids.length > 0 && ids.every((id) => current.oopzAccountIds.includes(id));
      return {
        ...current,
        oopzAccountIds: all ? [] : ids.slice(0, MAX_SHARED_PLATFORM_ACCOUNTS),
      };
    });
    if (ids.length > MAX_SHARED_PLATFORM_ACCOUNTS) {
      setShareSelectionNotice(`OOPZ 一次最多选择 ${MAX_SHARED_PLATFORM_ACCOUNTS} 个账号`);
    } else {
      setShareSelectionNotice("");
    }
  }

  function selectedSteamShareChoice(steamId?: string) {
    return steamId ? shareSelection.steamAccounts.find((account) => account.steamId === steamId) : undefined;
  }

  function setSteamShareCapability(steamId: string, capability: "webLogin" | "credential" | "perfect", checked: boolean) {
    const identity = shareableSteamIdentities.find((item) => item.steamId === steamId);
    if (!identity) return;
    setShareSelection((current) => {
      const existing = current.steamAccounts.find((account) => account.steamId === steamId);
      if (checked && !existing && current.steamAccounts.length >= MAX_SHARED_PLATFORM_ACCOUNTS) {
        setShareSelectionNotice(`Steam 一次最多选择 ${MAX_SHARED_PLATFORM_ACCOUNTS} 个账号`);
        return current;
      }
      setShareSelectionNotice("");
      const next: SteamShareChoice = existing
        ? { ...existing }
        : { steamId, webLogin: false, credential: false, perfect: false };
      next[capability] = checked;
      if (capability === "credential") {
        next.credentialFromPerfect = false;
      }
      if (capability === "perfect") {
        if (checked && identity.capabilities.credential && !next.credential) {
          next.credential = true;
          next.credentialFromPerfect = true;
        } else if (!checked && next.credentialFromPerfect) {
          next.credential = false;
          next.credentialFromPerfect = false;
        }
      }
      const steamAccounts = current.steamAccounts.filter((account) => account.steamId !== steamId);
      if (next.webLogin || next.credential || next.perfect) steamAccounts.push(next);
      return { ...current, steamAccounts };
    });
  }

  function perfectShareSelectionState(ids: string[]) {
    const selected = new Set(shareSelection.steamAccounts.filter((account) => account.perfect).map((account) => account.steamId));
    const selectedCount = ids.reduce((count, id) => count + Number(selected.has(id)), 0);
    return { any: selectedCount > 0, all: ids.length > 0 && selectedCount === ids.length };
  }

  function perfectShareCheckboxRef(element: HTMLInputElement | null, ids: string[]) {
    if (!element) return;
    const state = perfectShareSelectionState(ids);
    element.indeterminate = state.any && !state.all;
  }

  function steamCapabilitySelectionState(capability: "webLogin" | "credential") {
    const identities = shareableSteamIdentities.filter((identity) => identity.capabilities[capability]);
    const selectedCount = identities.reduce((count, identity) => {
      const choice = selectedSteamShareChoice(identity.steamId);
      const selected = capability === "webLogin"
        ? Boolean(choice?.webLogin || choice?.perfect)
        : Boolean(choice?.credential);
      return count + Number(selected);
    }, 0);
    return { any: selectedCount > 0, all: identities.length > 0 && selectedCount === identities.length };
  }

  function steamCapabilityCheckboxRef(element: HTMLInputElement | null, capability: "webLogin" | "credential") {
    if (!element) return;
    const state = steamCapabilitySelectionState(capability);
    element.indeterminate = state.any && !state.all;
  }

  function toggleSteamCapabilityBranch(capability: "webLogin" | "credential", checked: boolean) {
    const identities = shareableSteamIdentities.filter((identity) => identity.capabilities[capability]);
    setShareSelection((current) => {
      const byId = new Map(current.steamAccounts.map((account) => [account.steamId, { ...account }]));
      let capped = false;
      for (const identity of identities) {
        const steamId = identity.steamId || "";
        let choice = byId.get(steamId);
        if (!choice && checked && byId.size >= MAX_SHARED_PLATFORM_ACCOUNTS) {
          capped = true;
          continue;
        }
        choice ||= { steamId, webLogin: false, credential: false, perfect: false };
        choice[capability] = checked;
        if (capability === "credential") choice.credentialFromPerfect = false;
        if (choice.webLogin || choice.credential || choice.perfect) byId.set(steamId, choice);
        else byId.delete(steamId);
      }
      setShareSelectionNotice(capped ? `Steam 一次最多选择 ${MAX_SHARED_PLATFORM_ACCOUNTS} 个账号` : "");
      return { ...current, steamAccounts: Array.from(byId.values()) };
    });
  }

  function togglePerfectShareBranch(checked: boolean) {
    const ids = selectablePerfectIdentities.flatMap((identity) => identity.steamId ? [identity.steamId] : []);
    setShareSelection((current) => {
      const byId = new Map(current.steamAccounts.map((account) => [account.steamId, { ...account }]));
      for (const steamId of ids) {
        let choice = byId.get(steamId);
        if (!choice && checked && byId.size >= MAX_SHARED_PLATFORM_ACCOUNTS) continue;
        choice ||= { steamId, webLogin: false, credential: false, perfect: false };
        choice.perfect = checked;
        const identity = shareableSteamIdentities.find((item) => item.steamId === steamId);
        if (checked && identity?.capabilities.credential && !choice.credential) {
          choice.credential = true;
          choice.credentialFromPerfect = true;
        } else if (!checked && choice.credentialFromPerfect) {
          choice.credential = false;
          choice.credentialFromPerfect = false;
        }
        if (choice.webLogin || choice.credential || choice.perfect) byId.set(steamId, choice);
        else byId.delete(steamId);
      }
      if (checked && ids.some((id) => !byId.has(id))) {
        setShareSelectionNotice(`Steam 一次最多选择 ${MAX_SHARED_PLATFORM_ACCOUNTS} 个账号`);
      } else {
        setShareSelectionNotice("");
      }
      return { ...current, steamAccounts: Array.from(byId.values()) };
    });
  }

  function selectAllShareableAccounts() {
    const steamAccounts = shareableSteamIdentities.slice(0, MAX_SHARED_PLATFORM_ACCOUNTS).flatMap((identity) => identity.steamId ? [{
      steamId: identity.steamId,
      webLogin: identity.capabilities.webLogin,
      credential: identity.capabilities.credential,
      perfect: identity.capabilities.webLogin && isPerfectShareUsable(identity),
      credentialFromPerfect: false,
    }] : []);
    setShareSelection({
      oopzAccountIds: shareableOopzAccounts.slice(0, MAX_SHARED_PLATFORM_ACCOUNTS).map((account) => account.id),
      steamAccounts,
    });
    if (shareableOopzAccounts.length > MAX_SHARED_PLATFORM_ACCOUNTS || shareableSteamIdentities.length > MAX_SHARED_PLATFORM_ACCOUNTS) {
      setShareSelectionNotice(`每个平台一次最多选择 ${MAX_SHARED_PLATFORM_ACCOUNTS} 个账号`);
    } else {
      setShareSelectionNotice("");
    }
  }

  function setPerfectShareAvailableOnly(checked: boolean) {
    setSharePerfectAvailableOnly(checked);
    if (!checked) return;
    const usableIds = new Set(perfectShareIdentities.filter(isPerfectShareUsable).flatMap((identity) => identity.steamId ? [identity.steamId] : []));
    setShareSelection((current) => ({
      ...current,
      steamAccounts: current.steamAccounts.flatMap((account) => {
        if (!account.perfect || usableIds.has(account.steamId)) return [account];
        const next = { ...account, perfect: false };
        if (next.credentialFromPerfect) {
          next.credential = false;
          next.credentialFromPerfect = false;
        }
        return next.webLogin || next.credential ? [next] : [];
      }),
    }));
  }

  async function startQuickShare() {
    setQuickCode("");
    setQuickPackageBytes(null);
    setWormholeStatus({ state: "preparing", direction: "send", message: "正在准备快捷分享..." });
    try {
      const code = await invoke<string>("start_quick_export", { selection: shareSelection });
      setQuickCode(code);
      setMessage("快捷码已生成，等待对方接收");
    } catch (error) {
      const message = errorMessage(error);
      const cancelled = message.includes("已取消");
      setWormholeStatus({ state: cancelled ? "cancelled" : "error", direction: "send", message });
      setQuickCode("");
      setQuickPackageBytes(null);
      setMessage(message);
    } finally {
      refreshPerfectArena().catch(() => undefined);
    }
  }

  async function cancelQuickShare() {
    setWormholeStatus((current) => ({
      state: "cancelling",
      direction: current?.direction || "send",
      message: current?.direction === "receive" ? "正在取消导入..." : "正在取消分享...",
      code: current?.code,
    }));
    try {
      await invoke("cancel_quick_share");
    } catch (error) {
      const message = errorMessage(error);
      setWormholeStatus((current) => ({
        state: "error",
        direction: current?.direction || "send",
        message,
      }));
      setMessage(message);
    }
  }

  async function quickImport() {
    const code = receiveCode.trim();
    if (!code) {
      setMessage("请输入快捷码");
      return;
    }
    setWormholeStatus({ state: "connecting", direction: "receive", message: "正在连接发送方..." });
    try {
      const imported = await runTask("正在快捷导入...", () =>
        invoke<QuickImportResult>("quick_import", { code }),
      );
      setSelectedId(imported.oopzAccounts[0]?.id ?? null);
      setReceiveCode("");
      setMessage(formatQuickImportResult("快捷导入完成", imported));
    } finally {
      refreshPerfectArena().catch(() => undefined);
    }
  }

  function formatQuickImportResult(prefix: string, imported: QuickImportResult) {
    const credentialsRetained = Math.max(0, imported.steamCredentialsAccounts - imported.steamCredentialsAdded);
    const details = [
      imported.steamWebAccounts > 0 || imported.steamCredentialsAccounts > 0
        ? `Steam：网页态 ${imported.steamWebAccounts}，账密 ${imported.steamCredentialsAdded} 新增/${credentialsRetained} 已保留`
        : "",
      imported.perfectAccounts > 0 ? `完美平台 ${imported.perfectAdded} 新增/${imported.perfectUpdated} 更新` : "",
    ].filter(Boolean).join("，");
    return `${prefix}：OOPZ ${imported.oopzAccounts.length} 个${details ? `，${details}` : ""}`;
  }

  async function exportSharePackageFile() {
    if (selectedShareCount === 0) {
      setMessage("请至少选择一个可分享账号");
      return;
    }
    const target = await save({
      title: "导出跨平台账号分享包",
      defaultPath: `NEA_账号分享_${exportTimestamp()}.nea-share`,
      filters: [{ name: "NEA 跨平台分享包", extensions: ["nea-share"] }],
    });
    if (!target) return;
    setWormholeStatus({ state: "preparing", direction: "send", message: "正在导出跨平台分享包..." });
    try {
      const exported = await runTask("正在导出跨平台分享包...", () =>
        invoke<QuickShareExportResult>("export_quick_share_package_file", { selection: shareSelection, path: target }),
        false,
      );
      const message = `已导出 ${exported.accounts} 个账号，文件 ${formatFileSize(exported.packageBytes)}；请仅通过可信渠道传递`;
      setWormholeStatus({ state: "complete", direction: "send", message });
      setMessage(message);
    } catch (error) {
      const message = errorMessage(error);
      setWormholeStatus({ state: message.includes("已取消") ? "cancelled" : "error", direction: "send", message });
      throw error;
    } finally {
      refreshPerfectArena().catch(() => undefined);
    }
  }

  async function importSharePackageFile() {
    const source = await open({
      title: "导入跨平台账号分享包",
      multiple: false,
      filters: [{ name: "NEA 跨平台分享包", extensions: ["nea-share"] }],
    });
    if (!source || Array.isArray(source)) return;
    setWormholeStatus({ state: "importing", direction: "receive", message: "正在校验并导入跨平台分享包..." });
    try {
      const imported = await runTask("正在校验并导入跨平台分享包...", () =>
        invoke<QuickImportResult>("import_quick_share_package_file", { path: source }),
      );
      setSelectedId(imported.oopzAccounts[0]?.id ?? null);
      const message = formatQuickImportResult("分享包导入完成", imported);
      setWormholeStatus({ state: "complete", direction: "receive", message });
      setMessage(message);
    } catch (error) {
      const message = errorMessage(error);
      setWormholeStatus({ state: message.includes("已取消") ? "cancelled" : "error", direction: "receive", message });
      throw error;
    } finally {
      refreshPerfectArena().catch(() => undefined);
    }
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
    const result = await runTask("正在切换账号...", () =>
      invoke<{ ok: boolean; message: string }>("switch_account", { accountId: account.id }),
    );
    setMessage(result.message);
  }

  function requestQuickSwitch(event: MouseEvent<HTMLButtonElement>, account: SavedAccount) {
    setSelectedId(account.id);
    if (account.hasLoginState) {
      rememberDialogOpener(event);
      setPendingOopzOperation({ kind: "switch", account });
    } else {
      handleAction(() => switchAccount(account));
    }
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
    const result = await runTask("正在恢复备份...", () =>
      invoke<{ ok: boolean; message: string }>("restore_latest_backup"),
    );
    setMessage(result.message);
  }

  function requestRestoreBackup(event: MouseEvent<HTMLButtonElement>) {
    rememberDialogOpener(event);
    setPendingOopzOperation({ kind: "restore" });
  }

  function confirmOopzOperation() {
    if (!pendingOopzOperation || busy) return;
    const operation = pendingOopzOperation;
    handleAction(async () => {
      if (operation.kind === "switch") await switchAccount(operation.account);
      else await restoreBackup();
      setPendingOopzOperation(null);
    });
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

  async function refreshOopzAutoSignStatus() {
    const status = await invoke<OopzAutoSignStatus>("get_oopz_auto_sign_status");
    setOopzAutoSignStatus(status);
    return status;
  }

  async function toggleOopzAutoSign(enabled: boolean) {
    const status = await runTask(enabled ? "正在开启自动签到..." : "正在关闭自动签到...", () =>
      invoke<OopzAutoSignStatus>("set_oopz_auto_sign_enabled", { enabled }),
    );
    setOopzAutoSignStatus(status);
    setMessage(enabled ? "自动签到已开启" : "自动签到已关闭");
  }

  async function checkOopzAutoSignNow() {
    const status = await runTask("正在静默检查 OOPZ 签到状态...", () =>
      invoke<OopzAutoSignStatus>("check_oopz_auto_sign"),
    );
    setOopzAutoSignStatus(status);
    setMessage(status.message);
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
    const accountIds = await runTask(unavailable ? "正在从可用筛选中排除账号..." : "正在恢复到可用筛选...", () =>
      invoke<string[]>("set_perfect_account_unavailable", { steamId: session.steamId, unavailable }),
    );
    setData((current) => ({ ...current, perfectUnavailableAccountIds: accountIds }));
    setPerfectMenuSessionId(null);
    setMessage(unavailable ? "已从可用账号筛选和分享范围中排除" : "已恢复到可用账号筛选");
  }

  async function switchSteamAccount(identity: SteamIdentity) {
    if (!identity.steamId) return;
    const result = await runTask("正在使用已保存账密登录 Steam 客户端...", () => invoke<SwitchResult>("switch_steam_account", { accountId: identity.steamId }));
    setMessage(result.message);
  }

  function selectSteamIdentity(identity: SteamIdentity) {
    setSelectedSteamId((current) => current === identity.id ? null : identity.id);
    setSteamNoteDraft(identity.note || "");
  }

  async function saveSteamIdentityNote(identity: SteamIdentity) {
    const next = await runTask("正在保存 Steam 账号备注...", () => invoke<AppData>("set_steam_identity_note", { identityId: identity.id, note: steamNoteDraft }));
    setData(next);
    setMessage("Steam 账号备注已保存");
  }

  async function completeSteamCapabilities() {
    setSteamCapabilityStatus({ running: true, paused: false, cancelling: false });
    setSteamCapabilityResult(null);
    try {
      const result = await runTask("正在逐个检查并补全 Steam 网页登录...", () =>
        invoke<SteamCapabilityCompletionResult>("complete_steam_capabilities"),
      );
      const latest = await invoke<AppData>("get_app_data");
      setData(latest);
      setSteamCapabilityResult(result);
      const prefix = result.cancelled ? `网页登录补全已取消：已处理 ${result.processed}/${result.checked}` : `网页登录检查完成：检查 ${result.checked}`;
      setMessage(`${prefix}，无需处理 ${result.alreadyComplete}，新增网页登录 ${result.webCompleted}，需验证 ${result.verificationRequiredAccounts.length}，失败 ${result.failedAccounts.length}`);
    } finally {
      invoke<SteamCapabilityStatus>("get_steam_capability_status")
        .then(setSteamCapabilityStatus)
        .catch(() => setSteamCapabilityStatus({ running: false, paused: false, cancelling: false }));
    }
  }

  async function toggleSteamCapabilityPause() {
    try {
      const status = await invoke<SteamCapabilityStatus>("set_steam_capability_paused", { paused: !steamCapabilityStatus.paused });
      setSteamCapabilityStatus(status);
    } catch (error) {
      setMessage(errorMessage(error));
    }
  }

  async function cancelSteamCapabilityCompletion() {
    try {
      const status = await invoke<SteamCapabilityStatus>("cancel_steam_capability_completion");
      setSteamCapabilityStatus(status);
    } catch (error) {
      setMessage(errorMessage(error));
    }
  }

  async function refreshSteamUnified() {
    const warnings = await runTask("正在刷新 Steam 账号...", async () => {
      const nextWarnings: string[] = [];
      try {
        await invoke<SteamWorkspace>("discover_steam");
      } catch (error) {
        nextWarnings.push(`客户端：${errorMessage(error)}`);
      }
      if (data.steam.webSessions.length > 0) {
        try {
          await invoke<SteamWorkspace>("refresh_steam_web_sessions");
        } catch (error) {
          nextWarnings.push(`网页：${errorMessage(error)}`);
        }
      }
      return nextWarnings;
    });
    setMessage(warnings.length > 0 ? `Steam 账号已刷新；${warnings.join("；")}` : "Steam 账号已刷新");
  }

  async function optimizeStorage() {
    const result = await runTask("正在整理 NEA 存储并清理可重建缓存...", () =>
      invoke<StorageOptimizationResult>("optimize_storage"),
    );
    setMessage(`存储整理完成：释放 ${formatFileSize(result.freedBytes)}，优化 ${result.optimizedSessions} 个网页会话，清理 ${result.removedOrphanSessions} 个孤立目录，头像缓存 ${result.cachedPerfectAvatars} 个`);
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

  function parseSteamTextImport() {
    setSteamImportError("");
    const accounts: Array<{ account: string; password: string }> = [];
    const lines = steamTextImportDraft.split(/\r?\n/);
    for (let index = 0; index < lines.length; index += 1) {
      const line = lines[index].trim();
      if (!line) continue;
      const separator = line.search(/\s/);
      if (separator <= 0) {
        const issue = `第 ${index + 1} 行格式无效，请使用“账号 密码”格式`;
        setSteamImportError(issue);
        setMessage(issue);
        return null;
      }
      const account = line.slice(0, separator).trim();
      const password = line.slice(separator).trimStart();
      if (!account || !password) {
        const issue = `第 ${index + 1} 行缺少账号或密码`;
        setSteamImportError(issue);
        setMessage(issue);
        return null;
      }
      accounts.push({ account, password });
    }
    if (accounts.length === 0) {
      const issue = "没有可导入的 Steam 网页账号";
      setSteamImportError(issue);
      setMessage(issue);
      return null;
    }
    if (accounts.length > 100) {
      const issue = "一次最多导入 100 个 Steam 网页账号";
      setSteamImportError(issue);
      setMessage(issue);
      accounts.forEach((entry) => { entry.password = ""; });
      return null;
    }
    return accounts;
  }

  async function prepareSteamWebImport() {
    const accounts = parseSteamTextImport();
    if (!accounts) return;
    setSteamImportResult(null);
    try {
      const preview = await runTask("正在检查 Steam64 与重复账号...", () =>
        invoke<SteamImportPreview>("preview_steam_web_import", { accounts: accounts.map((entry) => entry.account) }),
      false);
      if (preview.existingAccounts.length > 0 || preview.duplicateInputAccounts.length > 0) {
        setSteamImportPreview(preview);
        setMessage(`导入前检查完成：已有网页登录 ${preview.existingAccounts.length}，重复输入 ${preview.duplicateInputAccounts.length}`);
        return;
      }
      await executeSteamWebImport(accounts, true);
    } finally {
      accounts.forEach((entry) => { entry.password = ""; });
    }
  }

  async function confirmSteamWebImport() {
    const accounts = parseSteamTextImport();
    if (!accounts) return;
    try {
      await executeSteamWebImport(accounts, true);
    } finally {
      accounts.forEach((entry) => { entry.password = ""; });
    }
  }

  function openSteamTextImportDialog(event?: MouseEvent<HTMLButtonElement>) {
    rememberDialogOpener(event);
    setSteamImportDraftState("");
    setShowSteamTextImport(true);
  }

  function promptDeleteAccount(event: MouseEvent<HTMLButtonElement>, account: SavedAccount) {
    rememberDialogOpener(event);
    setPendingDeleteAccount(account);
  }

  function promptDeleteSteamWebSession(event: MouseEvent<HTMLButtonElement>, session: SteamWebSession) {
    rememberDialogOpener(event);
    setPendingDeleteSteamWebSession(session);
  }

  function promptDeleteSteamCredential(event: MouseEvent<HTMLButtonElement>, identity: SteamIdentity) {
    rememberDialogOpener(event);
    setPendingDeleteSteamCredential(identity);
  }

  function closeSteamTextImportDialog() {
    if (busyRef.current) return;
    setShowSteamTextImport(false);
    setSteamTextImportDraft("");
    setSteamImportPreview(null);
    setSteamImportError("");
  }

  function setSteamImportDraftState(value: string) {
    setSteamTextImportDraft(value);
    setSteamImportPreview(null);
    setSteamImportError("");
  }

  async function cancelSteamWebImport() {
    if (!steamBulkImportRunning || steamBulkImportCancelling) return;
    setSteamBulkImportCancelling(true);
    setMessage("正在取消 Steam 网页账号导入...");
    try {
      const requested = await invoke<boolean>("cancel_steam_web_import");
      if (!requested) {
        setSteamBulkImportCancelling(false);
        setMessage("导入正在启动或已经结束；如仍在进行，可再次点击取消");
      }
    } catch (error) {
      setSteamBulkImportCancelling(false);
      setMessage(`取消导入失败：${errorMessage(error)}`);
    }
  }

  async function executeSteamWebImport(accounts: Array<{ account: string; password: string }>, skipExisting: boolean) {
    if (busyRef.current) throw new Error("已有操作正在进行，请稍候");
    setShowSteamTextImport(false);
    setSteamTextImportDraft("");
    setSteamImportPreview(null);
    setSteamImportError("");
    setSteamBulkImportRunning(true);
    setSteamBulkImportCancelling(false);
    try {
      const result = await runTask("正在批量导入 Steam 网页账号...", () =>
        invoke<SteamBulkImportResult>("import_steam_web_accounts_from_text", { accounts, skipExisting }),
      );
      const otherFailed = result.failedAccounts.length;
      setSteamImportResult(result);
      setMessage(`${result.cancelled ? "Steam 导入已取消" : "Steam 导入完成"}：成功 ${result.imported}，已有 ${result.skippedExisting}，密码错误 ${result.invalidCredentialAccounts.length}，有令牌 ${result.tokenProtectedAccounts.length}，需验证 ${result.verificationRequiredAccounts.length}，其他失败 ${otherFailed}${result.cancelled ? `，未处理 ${result.cancelled}` : ""}`);
    } finally {
      setSteamBulkImportRunning(false);
      setSteamBulkImportCancelling(false);
      accounts.forEach((entry) => { entry.password = ""; });
    }
  }

  async function openSteamWebSession(session: SteamWebSession) {
    await runTask("正在打开 Steam 网页账号...", () => invoke("open_steam_web_session", { sessionId: session.id }));
    setMessage(`已打开 ${session.displayName}`);
  }

  function selectSteamWebSession(session: SteamWebSession) {
    setPerfectMenuSessionId(null);
    setSelectedSteamWebSessionId((current) => current === session.id ? null : session.id);
    setSteamWebNoteDraft(session.note || "");
  }

  async function saveSteamWebSessionNote(session: SteamWebSession) {
    const workspace = await runTask("正在保存网页账号备注...", () => invoke<SteamWorkspace>("set_steam_web_session_note", { sessionId: session.id, note: steamWebNoteDraft }));
    setData((current) => ({ ...current, steam: workspace }));
    setMessage("Steam 网页账号备注已保存");
  }

  async function deleteSteamWebSession(session: SteamWebSession) {
    await runTask("正在删除 Steam 网页账号...", () => invoke("delete_steam_web_session", { sessionId: session.id }));
    setPendingDeleteSteamWebSession(null);
    if (selectedSteamWebSessionId === session.id) setSelectedSteamWebSessionId(null);
    setMessage("Steam 网页账号已删除");
  }

  async function deleteSteamCredential(identity: SteamIdentity) {
    const latest = await runTask("正在清除已保存的 Steam 账密...", () =>
      invoke<AppData>("delete_steam_saved_credential", { identityId: identity.id }),
    );
    setData(latest);
    setVisibleSteamCredentials([]);
    setPendingDeleteSteamCredential(null);
    setMessage("已清除保存的 Steam 账号密码；网页登录保持不变");
  }

  async function switchPerfectWebAccount(session: SteamWebSession) {
    const result = await runTask("正在通过 Steam 网页认证切换完美账号...", () => invoke<SwitchResult>("switch_perfect_web_account", { sessionId: session.id }));
    await refreshPerfectArena();
    setMessage(result.message);
  }

  async function switchSteamAndPerfectAccount(session: SteamWebSession) {
    const result = await runTask("正在同步切换 Steam 与完美账号...", () => invoke<SwitchResult>("switch_steam_and_perfect_account", { sessionId: session.id }));
    await refreshPerfectArena();
    setMessage(result.message);
  }

  function minimizeWindow() {
    setVisibleSteamCredentials([]);
    void getCurrentWindow().minimize().catch((error) => setMessage(errorMessage(error)));
  }

  function toggleMaximizeWindow() {
    void getCurrentWindow().toggleMaximize().catch((error) => setMessage(errorMessage(error)));
  }

  function closeWindow() {
    setVisibleSteamCredentials([]);
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
    document.documentElement.dataset.theme = darkMode ? "dark" : "light";
    writePreference("nea-theme", darkMode ? "dark" : "light");
  }, [darkMode]);

  useEffect(() => {
    writePreference("nea-active-app", activeApp);
    writePreference("nea-active-feature", activeFeature);
  }, [activeApp, activeFeature]);

  useEffect(() => {
    let disposed = false;
    const announceBootReady = () => window.dispatchEvent(new Event("nea:boot-ready"));
    refresh()
      .then(async () => {
        await invoke("get_config_health");
        if (disposed) return;
        setStartupPhase("ready");
        announceBootReady();
        if (activeApp === "oopz") {
          await validate().catch(() => setMessage("未找到 OOPZ，请在概览里手动选择目录"));
        } else {
          setMessage("NEA 已就绪");
        }
        await Promise.allSettled([
          invoke<UpdateStatus>("get_update_status").then((status) => {
            setUpdateStatus(status);
            if (status.state === "updated" || status.state === "error") setMessage(status.message);
          }),
          invoke<SteamCapabilityStatus>("get_steam_capability_status").then(setSteamCapabilityStatus),
          refreshOopzAutoSignStatus(),
          refreshPluginStatus(),
          refreshPerfectArena(),
        ]);
      })
      .catch((error) => {
        if (disposed) return;
        const startupMessage = errorMessage(error);
        setStartupError(startupMessage);
        setStartupPhase("error");
        setMessage(startupMessage);
        announceBootReady();
      });

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
    keepListener(listen<boolean>("steam-bulk-import-state", (event) => {
      setSteamBulkImportRunning(event.payload);
      if (!event.payload) setSteamBulkImportCancelling(false);
    }));
    keepListener(listen<string>("steam-client-switch-progress", (event) => {
      setMessage(event.payload);
    }));
    keepListener(listen<string>("steam-capability-progress", (event) => {
      setMessage(event.payload);
    }));
    keepListener(listen<SteamCapabilityStatus>("steam-capability-state", (event) => {
      setSteamCapabilityStatus(event.payload);
    }));
    keepListener(listen<StorageOptimizationResult>("storage-optimized", (event) => {
      if (event.payload.freedBytes > 0) {
        setMessage(`后台存储整理完成，已释放 ${formatFileSize(event.payload.freedBytes)}`);
      }
    }));
    keepListener(listen<string>("plugin-environment-finished", (event) => {
      refreshPluginStatus().catch(() => undefined);
      if (event.payload) setMessage(event.payload);
    }));
    keepListener(listen<OopzAutoSignStatus>("oopz-auto-sign-status", (event) => {
      setOopzAutoSignStatus(event.payload);
      if (event.payload.state === "signed" || event.payload.state === "error") {
        setMessage(event.payload.message);
      }
    }));
    keepListener(listen<UpdateStatus>("update-status", (event) => {
      setUpdateStatus(event.payload);
      setMessage(event.payload.message);
    }));
    keepListener(listen<WormholeStatus>("wormhole-status", (event) => {
      setWormholeStatus(event.payload);
      if (event.payload.code) setQuickCode(event.payload.code);
      if (typeof event.payload.packageBytes === "number") setQuickPackageBytes(event.payload.packageBytes);
      if (["cancelled", "complete", "error"].includes(event.payload.state)) {
        setQuickCode("");
        setQuickPackageBytes(null);
      }
      if (event.payload.direction === "receive" && event.payload.state === "complete") {
        setReceiveCode("");
      }
      setMessage(event.payload.message);
    }));
    return () => {
      disposed = true;
      unsubs.forEach((unsub) => unsub());
    };
  }, []);

  useEffect(() => {
    if (startupPhase !== "ready") return;
    if (activeApp === "oopz" && activeFeature === "switcher" && !scannedOnceRef.current) {
      refreshAccounts(false).catch(() => undefined);
    }
    if (activeApp === "oopz" && activeFeature === "switcher") {
      refreshPluginStatus().catch(() => undefined);
    }
    if (activeApp === "oopz" && activeFeature === "autoSign") {
      refreshOopzAutoSignStatus().catch(() => undefined);
    }
    if (activeApp === "perfect") {
      refreshPerfectArena().catch(() => undefined);
    }
    setVisibleSteamCredentials([]);
  }, [activeApp, activeFeature, startupPhase]);

  useEffect(() => {
    if (!pendingOopzOperation && !pendingDeleteAccount && !pendingDeleteSteamWebSession && !pendingDeleteSteamCredential && !showSteamTextImport && !showShareCenter && !perfectMenuSessionId) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !busy) {
        setPendingOopzOperation(null);
        setPendingDeleteAccount(null);
        setPendingDeleteSteamWebSession(null);
        setPendingDeleteSteamCredential(null);
        setShowSteamTextImport(false);
        setSteamTextImportDraft("");
        setSteamImportPreview(null);
        setSteamImportError("");
        setPerfectMenuSessionId(null);
        if (!wormholeActive) setShowShareCenter(false);
      }
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [pendingOopzOperation, pendingDeleteAccount, pendingDeleteSteamWebSession, pendingDeleteSteamCredential, showSteamTextImport, showShareCenter, perfectMenuSessionId, wormholeActive, busy]);

  useEffect(() => {
    const rememberPageFocus = (event: Event) => {
      const eventTarget = event.target;
      if (!(eventTarget instanceof Element)) return;
      const target = eventTarget.closest<HTMLElement>('button, input, select, textarea, [href], [tabindex]');
      if (target && !target.closest('[aria-modal="true"]')) {
        lastPageFocusRef.current = target;
      }
    };
    document.addEventListener("focusin", rememberPageFocus);
    document.addEventListener("pointerdown", rememberPageFocus, true);
    return () => {
      document.removeEventListener("focusin", rememberPageFocus);
      document.removeEventListener("pointerdown", rememberPageFocus, true);
    };
  }, []);

  useEffect(() => {
    if (!activeDialogKey) return;
    const dialogSelector = '.confirm-dialog[aria-modal="true"], .share-center[aria-modal="true"]';
    const focusableSelector = 'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [href], [tabindex]:not([tabindex="-1"])';
    const focusDialog = window.requestAnimationFrame(() => {
      const dialog = document.querySelector<HTMLElement>(dialogSelector);
      if (!dialog || dialog.contains(document.activeElement)) return;
      (dialog.querySelector<HTMLElement>("[autofocus]") || dialog.querySelector<HTMLElement>(focusableSelector) || dialog).focus();
    });
    const trapFocus = (event: KeyboardEvent) => {
      if (event.key !== "Tab") return;
      const dialog = document.querySelector<HTMLElement>(dialogSelector);
      if (!dialog) return;
      const focusable = Array.from(dialog.querySelectorAll<HTMLElement>(focusableSelector));
      if (focusable.length === 0) {
        event.preventDefault();
        dialog.focus();
        return;
      }
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && (document.activeElement === first || !dialog.contains(document.activeElement))) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && (document.activeElement === last || !dialog.contains(document.activeElement))) {
        event.preventDefault();
        first.focus();
      }
    };
    document.addEventListener("keydown", trapFocus);
    return () => {
      window.cancelAnimationFrame(focusDialog);
      document.removeEventListener("keydown", trapFocus);
      window.requestAnimationFrame(() => {
        if (!document.querySelector(dialogSelector)) lastPageFocusRef.current?.focus();
      });
    };
  }, [activeDialogKey]);

  useEffect(() => {
    if (visibleSteamCredentials.length === 0) return;
    const hideCredentials = () => setVisibleSteamCredentials([]);
    const hideWhenBackgrounded = () => {
      if (document.hidden) hideCredentials();
    };
    const timer = window.setTimeout(hideCredentials, 15_000);
    window.addEventListener("blur", hideCredentials);
    document.addEventListener("visibilitychange", hideWhenBackgrounded);
    return () => {
      window.clearTimeout(timer);
      window.removeEventListener("blur", hideCredentials);
      document.removeEventListener("visibilitychange", hideWhenBackgrounded);
    };
  }, [visibleSteamCredentials]);

  useEffect(() => {
    if (!perfectMenuSessionId) return;
    const closeMenuOutside = (event: Event) => {
      const target = event.target as HTMLElement | null;
      if (!target?.closest(".perfect-card-menu-wrap")) setPerfectMenuSessionId(null);
    };
    window.addEventListener("mousedown", closeMenuOutside);
    return () => window.removeEventListener("mousedown", closeMenuOutside);
  }, [perfectMenuSessionId]);

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
          <button onClick={requestRestoreBackup} disabled={busy}>恢复最近备份</button>
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
                    <button className="icon-button danger" onClick={(event) => promptDeleteAccount(event, account)} disabled={busy || account.uid === data.currentLoginUid} aria-label={`删除 ${account.displayName} 及其本地登录态`} title={account.uid === data.currentLoginUid ? "当前登录账号不能删除，请先切换或退出" : "删除账号及本地登录态"}><Trash2 size={16} strokeWidth={2} /></button>
                    <button onClick={() => handleAction(() => exportSelectedAccount(account))} disabled={busy || !account.hasLoginState}>导出</button>
                    <button className={account.hasLoginState ? "primary" : ""} onClick={(event) => requestQuickSwitch(event, account)} disabled={busy || account.uid === data.currentLoginUid}>{account.uid === data.currentLoginUid ? "当前登录" : account.hasLoginState ? "快速切号" : "登录一次"}</button>
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
              <button data-active={!data.config.overlayVertical} aria-pressed={!data.config.overlayVertical} onClick={() => handleAction(() => setOverlayLayout(false))} disabled={busy}>横排</button>
              <button data-active={Boolean(data.config.overlayVertical)} aria-pressed={Boolean(data.config.overlayVertical)} onClick={() => handleAction(() => setOverlayLayout(true))} disabled={busy}>竖排</button>
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

  const autoSignAccount = oopzAutoSignStatus?.accountUid
    ? data.accounts.find((account) => account.uid === oopzAutoSignStatus.accountUid)
    : undefined;
  const autoSignStateLabel = oopzAutoSignStatus?.state === "signed" ? "今日已完成"
    : oopzAutoSignStatus?.state === "checking" ? "检查中"
      : oopzAutoSignStatus?.state === "waiting" ? "等待登录"
        : oopzAutoSignStatus?.state === "error" ? "稍后重试"
          : "未开启";
  const autoSign = (
    <section className="content-stack auto-sign-page">
      <div className="panel auto-sign-hero" data-state={oopzAutoSignStatus?.state || "disabled"}>
        <div className="auto-sign-icon" aria-hidden="true"><CalendarCheck size={28} /></div>
        <div className="auto-sign-heading">
          <h2>自动签到</h2>
        </div>
        <button
          className={oopzAutoSignStatus?.enabled ? "" : "primary"}
          onClick={() => handleAction(() => toggleOopzAutoSign(!oopzAutoSignStatus?.enabled))}
          disabled={busy || oopzAutoSignStatus?.state === "checking"}
        >{oopzAutoSignStatus?.enabled ? "关闭" : "开启"}</button>
      </div>

      <div className="summary-grid auto-sign-summary">
        <div className="metric"><strong>{autoSignStateLabel}</strong><span>今日状态</span></div>
        <div className="metric"><strong>{oopzAutoSignStatus?.accumulatedDays ?? "-"}</strong><span>累计签到天数</span></div>
        <div className="metric"><strong>{oopzAutoSignStatus?.freeCoinBalance ?? "-"}</strong><span>抽奖币余额</span></div>
      </div>

      <div className="panel auto-sign-detail">
        <div className="panel-title">
          <h2>运行状态</h2>
          <button onClick={() => handleAction(checkOopzAutoSignNow)} disabled={busy || !oopzAutoSignStatus?.enabled || oopzAutoSignStatus?.state === "checking"}>
            {oopzAutoSignStatus?.state === "checking" ? "检查中" : "立即检查"}
          </button>
        </div>
        <div className="auto-sign-status-line" data-state={oopzAutoSignStatus?.state || "disabled"}>
          <span className="auto-sign-status-dot" />
          <strong>{oopzAutoSignStatus?.message || "正在读取自动签到状态"}</strong>
        </div>
        <dl className="meta auto-sign-meta">
          <dt>当前账号</dt><dd>{autoSignAccount?.displayName || (oopzAutoSignStatus?.accountUid ? "当前 OOPZ 账号" : "等待 OOPZ 登录")}</dd>
          <dt>最近检查</dt><dd>{fmtDate(oopzAutoSignStatus?.lastCheckedAt)}</dd>
          <dt>最近签到</dt><dd>{fmtDate(oopzAutoSignStatus?.lastSignedAt)}</dd>
          <dt>签到奖励</dt><dd>{oopzAutoSignStatus?.rewardName ? `${oopzAutoSignStatus.rewardName}${oopzAutoSignStatus.rewardQuantity ? ` ×${oopzAutoSignStatus.rewardQuantity}` : ""}` : "-"}</dd>
        </dl>
      </div>
    </section>
  );

  const steamOverview = (
    <section className="content-stack">
      <div className="panel">
        <div className="panel-title"><h2>Steam 状态</h2><div className="actions"><button onClick={() => handleAction(optimizeStorage)} disabled={busy}>整理存储</button></div></div>
        <dl className="paths">
          <dt>程序</dt><dd>{data.steam.installation?.executable || "未设置"}</dd>
          <dt>安装目录</dt><dd>{data.steam.installation?.installDir || "未设置"}</dd>
        </dl>
      </div>
    </section>
  );

  const steamSwitcher = (
    <section className="content-stack">
      <div className="panel steam-unified-panel">
        <div className="panel-title steam-unified-title">
          <h2>Steam 账号</h2>
          <div className="actions">
            <button onClick={() => handleAction(refreshSteamUnified)} disabled={busy}>刷新</button>
            {steamBulkImportRunning
              ? <button className="primary danger-confirm" onClick={() => void cancelSteamWebImport()} disabled={steamBulkImportCancelling}>{steamBulkImportCancelling ? "正在取消..." : "取消本次导入"}</button>
              : <>
                <button data-testid="steam-text-import-open" onClick={openSteamTextImportDialog} disabled={busy}>从文本导入</button>
                {steamCapabilityStatus.running
                  ? <><button onClick={() => void toggleSteamCapabilityPause()} disabled={steamCapabilityStatus.cancelling}>{steamCapabilityStatus.paused ? "继续补全" : "暂停补全"}</button><button className="primary danger-confirm" onClick={() => void cancelSteamCapabilityCompletion()} disabled={steamCapabilityStatus.cancelling}>{steamCapabilityStatus.cancelling ? "取消中" : "取消补全"}</button></>
                  : <button onClick={() => handleAction(completeSteamCapabilities)} disabled={busy || !(data.steamCredentials?.length)}>检查并补全网页登录</button>}
                <button className="primary" onClick={() => handleAction(createSteamWebSession)} disabled={busy}>新增网页登录</button>
              </>}
          </div>
        </div>
        {steamBulkImportRunning && <aside className="steam-import-active" role="status" aria-live="polite"><RefreshCw className="spin-icon" size={18} aria-hidden="true" /><span><strong>{steamBulkImportCancelling ? "正在安全停止导入" : "正在批量建立网页登录"}</strong><small>{steamBulkImportCancelling ? "已打开的登录窗口会关闭，未处理账号不会保存。" : "识别成功、密码错误或令牌状态后会自动继续下一个账号。"}</small></span></aside>}
        {steamImportResult && <aside className="steam-import-result" aria-live="polite">
          <header><div><strong>{steamImportResult.cancelled ? "上次导入已取消" : "上次导入结果"}</strong><span>成功 {steamImportResult.imported} · 已有 {steamImportResult.skippedExisting} · 重复输入 {steamImportResult.skippedDuplicateInput}</span></div><button className="icon-button" onClick={() => setSteamImportResult(null)} aria-label="关闭导入结果"><X size={16} /></button></header>
          <div className="steam-import-result-metrics">
            <span data-state="success"><strong>{steamImportResult.imported}</strong>成功</span>
            <span data-state={steamImportResult.invalidCredentialAccounts.length ? "error" : undefined}><strong>{steamImportResult.invalidCredentialAccounts.length}</strong>密码错误</span>
            <span data-state={steamImportResult.tokenProtectedAccounts.length ? "warning" : undefined}><strong>{steamImportResult.tokenProtectedAccounts.length}</strong>有令牌</span>
            <span data-state={steamImportResult.verificationRequiredAccounts.length ? "warning" : undefined}><strong>{steamImportResult.verificationRequiredAccounts.length}</strong>需邮件验证</span>
            <span data-state={steamImportOtherFailed ? "error" : undefined}><strong>{steamImportOtherFailed}</strong>其他失败</span>
            {steamImportResult.cancelled > 0 && <span><strong>{steamImportResult.cancelled}</strong>未处理</span>}
          </div>
          {steamImportAttentionAccounts.length > 0 && <details>
            <summary>查看 {steamImportAttentionAccounts.length} 个需处理账号</summary>
            {steamImportResult.invalidCredentialAccounts.length > 0 && <div className="steam-import-account-list"><strong>密码错误：</strong>{steamImportResult.invalidCredentialAccounts.join("、")}</div>}
            {steamImportResult.tokenProtectedAccounts.length > 0 && <div className="steam-import-account-list"><strong>有令牌：</strong>{steamImportResult.tokenProtectedAccounts.join("、")}</div>}
            {steamImportResult.verificationRequiredAccounts.length > 0 && <div className="steam-import-account-list"><strong>需邮件验证：</strong>{steamImportResult.verificationRequiredAccounts.join("、")}</div>}
            {steamImportResult.failedAccounts.length > 0 && <div className="steam-import-account-list"><strong>其他失败：</strong>{steamImportResult.failedAccounts.join("、")}</div>}
            {steamImportResult.cancelledAccounts.length > 0 && <div className="steam-import-account-list"><strong>未处理：</strong>{steamImportResult.cancelledAccounts.join("、")}</div>}
            <button onClick={() => void copyText(steamImportAttentionAccounts.join("\n"))}>复制账号列表</button>
          </details>}
        </aside>}
        {steamCapabilityResult && <aside className="steam-import-result steam-capability-result" aria-live="polite">
          <header><div><strong>{steamCapabilityResult.cancelled ? "网页登录补全已取消" : "网页登录补全结果"}</strong><span>已处理 {steamCapabilityResult.processed}/{steamCapabilityResult.checked}</span></div><button className="icon-button" onClick={() => setSteamCapabilityResult(null)} aria-label="关闭补全结果"><X size={16} /></button></header>
          <div className="steam-import-result-metrics">
            <span><strong>{steamCapabilityResult.checked}</strong>检查账号</span>
            <span data-state="success"><strong>{steamCapabilityResult.webCompleted}</strong>新增网页</span>
            <span><strong>{steamCapabilityResult.alreadyComplete}</strong>原本完整</span>
            <span data-state={steamCapabilityResult.verificationRequiredAccounts.length ? "warning" : undefined}><strong>{steamCapabilityResult.verificationRequiredAccounts.length}</strong>需验证</span>
            <span data-state={steamCapabilityResult.failedAccounts.length ? "error" : undefined}><strong>{steamCapabilityResult.failedAccounts.length}</strong>失败</span>
          </div>
          {(steamCapabilityResult.verificationRequiredAccounts.length > 0 || steamCapabilityResult.failedAccounts.length > 0) && <details><summary>查看需处理账号</summary>{steamCapabilityResult.verificationRequiredAccounts.length > 0 && <div className="steam-import-account-list"><strong>需验证：</strong>{steamCapabilityResult.verificationRequiredAccounts.join("、")}</div>}{steamCapabilityResult.failedAccounts.length > 0 && <div className="steam-import-account-list"><strong>失败：</strong>{steamCapabilityResult.failedAccounts.join("、")}</div>}<button onClick={() => void copyText([...steamCapabilityResult.verificationRequiredAccounts, ...steamCapabilityResult.failedAccounts].join("\n"))}>复制账号列表</button></details>}
        </aside>}
        {pendingSteamWebSessions.length > 0 && <aside className="steam-pending-sessions">
          <header><strong>未完成的网页登录</strong><span>{pendingSteamWebSessions.length} 个</span></header>
          {pendingSteamWebSessions.map((session) => { const label = session.accountName || session.displayName || "待登录 Steam 账号"; return <div key={session.id}><span><strong>{label}</strong><small>尚未识别 SteamID64，可继续登录或删除此次会话。</small></span><button onClick={() => handleAction(() => openSteamWebSession(session))} disabled={busy}>继续登录</button><button className="icon-button danger" onClick={(event) => promptDeleteSteamWebSession(event, session)} disabled={busy} aria-label={`删除 ${label} 的未完成网页登录`} title="删除未完成会话"><Trash2 size={16} /></button></div>; })}
        </aside>}
        <div className="steam-filter-bar">
          <input type="search" value={steamSearch} onChange={(event) => setSteamSearch(event.target.value)} placeholder="搜索 Steam 名称、ID、账号或完美ID" aria-label="搜索 Steam 账号" />
          <select value={steamWebFilter} onChange={(event) => setSteamWebFilter(event.target.value)} aria-label="筛选网页登录"><option value="all">全部网页状态</option><option value="yes">有网页登录</option><option value="no">无网页登录</option></select>
          <select value={steamClientFilter} onChange={(event) => setSteamClientFilter(event.target.value)} aria-label="筛选客户端账密"><option value="all">全部客户端账密</option><option value="yes">可账密登录客户端</option><option value="no">缺少客户端账密</option></select>
        </div>
        <div className="account-list steam-identity-list auto-hide-scrollbar" onScroll={showScrollbarWhileScrolling}>
          {filteredSteamIdentities.length === 0 && <div className="empty actionable-empty"><strong>{steamIdentityCount === 0 ? "还没有 Steam 账号" : "没有符合筛选条件的账号"}</strong><span>{steamIdentityCount === 0 ? "添加网页登录，或批量导入账号密码后开始管理。" : "调整搜索词和状态筛选，或一键清空筛选。"}</span><div className="actions">{steamIdentityCount === 0 ? <><button className="primary" onClick={() => handleAction(createSteamWebSession)} disabled={busy}>新增网页登录</button><button onClick={openSteamTextImportDialog} disabled={busy}>从文本导入</button></> : steamFiltersActive && <button onClick={resetSteamFilters}>清空筛选</button>}</div></div>}
          {filteredSteamIdentities.map((identity) => {
            const session = identity.webSessionId
              ? data.steam.webSessions.find((item) => item.id === identity.webSessionId)
              : data.steam.webSessions.find((item) => Boolean(identity.steamId && item.steamId === identity.steamId));
            const credential = findSteamCredential(identity.steamId, identity.accountName, identity.webSessionId, identity.clientAccountId);
            const current = Boolean(identity.steamId && data.steam.currentAccountId === identity.steamId);
            const clientOnline = current && data.steam.clientOnline;
            const profile = identity.steamId ? perfectProfiles[identity.steamId] : undefined;
            const numericFallback = Boolean(identity.steamId && identity.displayName.trim() === identity.steamId);
            const accountNameFallback = Boolean(
              identity.accountName
              && identity.displayName.trim().toLocaleLowerCase() === identity.accountName.trim().toLocaleLowerCase(),
            );
            const label = !numericFallback && !accountNameFallback && identity.displayName.trim()
              ? identity.displayName
              : identity.steamId ? `Steam ${identity.steamId}` : "Steam 账号";
            return <div className="account-row steam-identity-row" data-selected={current || selectedSteamId === identity.id} key={identity.id}>
              <div className="account-row-main">
                <button className="account-main" onClick={() => selectSteamIdentity(identity)} aria-expanded={selectedSteamId === identity.id}><span className="avatar-wrap"><span className="avatar-fallback">S</span></span><span><strong>{label}</strong><span className="steam-identity-meta"><small>{identity.steamId ? `SteamID64: ${identity.steamId}` : "缺乏 SteamID64"}</small><small>完美ID: {profile?.nickname || "未检测"}</small>{identity.note && <small>备注: {identity.note}</small>}</span>{steamLoginMethods(identity)}</span></button>
                <div className="account-actions steam-identity-actions">
                  {credential && credentialEye(credential, label)}
                  {credential && <button className="icon-button danger" onClick={(event) => promptDeleteSteamCredential(event, identity)} disabled={busy} aria-label={`清除 ${label} 保存的账号密码`} title="清除已保存账密（保留网页登录）"><KeyRound size={16} /></button>}
                  {session && <button className="icon-button danger" onClick={(event) => promptDeleteSteamWebSession(event, session)} disabled={busy} aria-label={`删除 ${label} 的网页登录`} title="删除网页登录"><Trash2 size={16} /></button>}
                  <button onClick={() => session && handleAction(() => openSteamWebSession(session))} disabled={busy || !session}>{session ? "打开网页" : "网页未登录"}</button>
                  <button className={credential && identity.steamId && !clientOnline ? "primary" : ""} onClick={() => identity.steamId && credential && handleAction(() => switchSteamAccount(identity))} disabled={busy || !identity.steamId || !credential || clientOnline}>{clientOnline ? "客户端已上线" : current && credential ? "等待客户端上线" : credential && identity.steamId ? "账密打开客户端" : credential ? "待识别 SteamID" : "缺少账密"}</button>
                </div>
              </div>
              {credential && credentialDetails(credential)}
              {selectedSteamId === identity.id && <div className="steam-account-note"><input value={steamNoteDraft} onChange={(event) => setSteamNoteDraft(event.target.value)} maxLength={120} placeholder="添加账号备注" /><button onClick={() => handleAction(() => saveSteamIdentityNote(identity))} disabled={busy}>保存备注</button></div>}
            </div>;
          })}
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
        <div className="actions"><button onClick={() => handleAction(refreshPerfectProfiles)} disabled={busy || data.steam.webSessions.every((session) => !session.steamId)}>刷新资料</button><button className="primary" onClick={() => handleAction(createSteamWebSession)} disabled={busy}>添加 Steam 网页账号</button></div>
      </header>
      <div className="perfect-filter-bar">
        <input type="search" value={perfectSearch} onChange={(event) => setPerfectSearch(event.target.value)} placeholder="搜索 ID、名称或备注" aria-label="搜索完美账号" />
        <select value={perfectScoreFilter} onChange={(event) => { const value = event.target.value; setPerfectScoreFilter(value); if (value === "pending") setPerfectAvailableOnly(false); if (value !== "all" && value !== "pending") setPerfectPendingOnly(false); }} aria-label="按段位筛选">
          <option value="all">全部段位</option>
          <option value="pending">段位待检测</option>
          <option value="D">D</option>
          <option value="C">C</option>
          <option value="C+">C+</option>
          <option value="金C+">金C+</option>
          <option value="B">B</option>
          <option value="B+">B+</option>
          <option value="金B+">金B+</option>
          <option value="A">A</option>
          <option value="A+">A+</option>
          <option value="金A+">金A+</option>
        </select>
        <label><input type="checkbox" checked={perfectPendingOnly} onChange={(event) => { const checked = event.target.checked; setPerfectPendingOnly(checked); if (checked) { setPerfectAvailableOnly(false); if (perfectScoreFilter !== "all" && perfectScoreFilter !== "pending") setPerfectScoreFilter("all"); } }} />仅显示待检测</label>
        <label><input type="checkbox" checked={perfectAvailableOnly} onChange={(event) => { const checked = event.target.checked; setPerfectAvailableOnly(checked); if (checked) { setPerfectPendingOnly(false); if (perfectScoreFilter === "pending") setPerfectScoreFilter("all"); } }} />仅显示可用账号</label>
      </div>
      <div className="perfect-account-grid auto-hide-scrollbar" onScroll={showScrollbarWhileScrolling}>
        {data.steam.webSessions.length === 0 && <div className="empty perfect-grid-empty actionable-empty"><strong>还没有可关联的 Steam 网页账号</strong><span>完美平台通过 Steam 网页身份读取并切换账号。</span><div className="actions"><button className="primary" onClick={() => handleAction(createSteamWebSession)} disabled={busy}>添加 Steam 网页账号</button></div></div>}
        {data.steam.webSessions.length > 0 && filteredPerfectSessions.length === 0 && <div className="empty perfect-grid-empty actionable-empty"><strong>没有符合筛选条件的账号</strong><span>“待检测”和“可用账号”不会同时生效。</span>{perfectFiltersActive && <div className="actions"><button onClick={resetPerfectFilters}>清空筛选</button></div>}</div>}
        {filteredPerfectSessions.map((session) => {
          const current = Boolean(session.steamId && session.steamId === perfectWorkspace.currentAccountId);
          const profile = session.steamId ? perfectProfiles[session.steamId] : undefined;
          const identity = findSteamIdentity(session.steamId, session.accountName, session.id);
          const credential = findSteamCredential(session.steamId, session.accountName, session.id);
          const steamAlreadyCurrent = Boolean(session.steamId && data.steam.currentAccountId === session.steamId);
          const steamAlreadyOnline = steamAlreadyCurrent && data.steam.clientOnline;
          const canSyncSteam = Boolean(credential || steamAlreadyOnline);
          const selected = selectedSteamWebSessionId === session.id;
          const requiresVerification = Boolean(profile?.highRisk || profile?.reputationRequiresVerification);
          const unavailable = Boolean(session.steamId && data.perfectUnavailableAccountIds?.includes(session.steamId));
          const reputationLabel = unavailable ? "不可用" : requiresVerification ? "高危" : profile?.reputationLevel || "待检测";
          const reputationDetail = unavailable ? null : requiresVerification ? "需验证" : profile?.reputationPoints;
          return (
            <article className="perfect-account-card" data-current={current} data-selected={selected} data-high-risk={requiresVerification || unavailable || undefined} key={session.id}>
              {session.steamId && <div className="perfect-card-menu-wrap">
                <button className="icon-button perfect-card-menu-button" onClick={() => setPerfectMenuSessionId((value) => value === session.id ? null : session.id)} aria-label={`${profile?.nickname || session.displayName} 的账号操作`} aria-expanded={perfectMenuSessionId === session.id} title="账号操作"><MoreHorizontal size={17} /></button>
                {perfectMenuSessionId === session.id && <div className="perfect-card-menu">
                  <button onClick={() => handleAction(() => setPerfectAccountUnavailable(session, !unavailable))}>{unavailable ? "恢复到可用筛选" : "从可用筛选中排除"}</button>
                </div>}
              </div>}
              <button className="perfect-card-identity" onClick={() => selectSteamWebSession(session)} aria-expanded={selected}>
                <span className="perfect-card-avatar">{profile?.avatarUrl ? <img src={profile.avatarUrl} alt="" referrerPolicy="no-referrer" /> : <span className="avatar-fallback">P</span>}</span>
                <span className="perfect-card-name"><strong>{profile?.nickname || identity?.displayName || session.accountName || session.displayName}</strong><small>{identity?.steamId || session.steamId || "等待登录识别"}</small>{identityCapabilityBadges(identity)}</span>
                {current && <span className="perfect-current-badge">当前</span>}
              </button>
              <div className="perfect-card-metrics">
                <span><small>等级分</small><strong>{perfectScoreLabel(profile?.score)}</strong></span>
                <span><small>玩家身份</small><strong>{profile?.playerIdentity || "待检测"}</strong></span>
                <span className="perfect-reputation" data-level={requiresVerification || unavailable ? "danger" : profile?.reputationLevel || "pending"}><small>信誉等级</small><strong>{reputationLabel}{reputationDetail != null ? <em>{reputationDetail}</em> : null}</strong></span>
              </div>
              {(selected || Boolean(credential && credentialVisible(credential))) && <div className="perfect-card-overlay-details">
                {selected && <div className="steam-account-note"><input value={steamWebNoteDraft} onChange={(event) => setSteamWebNoteDraft(event.target.value)} maxLength={120} placeholder="添加账号备注" /><button onClick={() => handleAction(() => saveSteamWebSessionNote(session))} disabled={busy}>保存</button></div>}
                {credential && credentialDetails(credential)}
              </div>}
              <div className="perfect-card-actions" data-has-credential={Boolean(credential) || undefined}>
                {credential && credentialEye(credential, profile?.nickname || session.displayName)}
                <button className="icon-button danger" onClick={(event) => promptDeleteSteamWebSession(event, session)} disabled={busy} aria-label={`删除 ${session.displayName} 的网页登录`} title="删除网页登录（保留已保存账密）"><Trash2 size={16} /></button>
                <button className={canSyncSteam && !(current && steamAlreadyOnline) ? "primary" : ""} onClick={() => handleAction(() => switchSteamAndPerfectAccount(session))} disabled={busy || !canSyncSteam || (current && steamAlreadyOnline) || !perfectWorkspace.installation || !data.steam.installation} title={steamAlreadyOnline ? "Steam 客户端已在线，仅同步切换完美账号" : credential ? "使用已保存账密登录 Steam 客户端，并切换完美账号" : "该账号没有已保存账号密码"}>同步切换</button>
                {session.steamId
                  ? <button onClick={() => handleAction(() => switchPerfectWebAccount(session))} disabled={busy || current || !perfectWorkspace.installation}>{current ? "完美已登录" : "仅切完美"}</button>
                  : <button onClick={() => handleAction(() => openSteamWebSession(session))} disabled={busy}>登录</button>}
              </div>
            </article>
          );
        })}
      </div>
    </section>
  );

  function selectApp(app: AppKey) {
    const changingApp = app !== activeApp;
    setActiveApp(app);
    if (changingApp || (app !== "oopz" && activeFeature === "autoSign")) setActiveFeature("switcher");
    setVisibleSteamCredentials([]);
    setPerfectMenuSessionId(null);
    if (busyRef.current) return;
    if (app === "oopz") {
      setMessage(`已保存 ${data.accounts.length} 个 OOPZ 账号，${sessionCount} 个可快速切换`);
    } else if (app === "steam") {
      setMessage(`Steam 网页账号 ${data.steam.webSessions.length} 个，客户端账号 ${data.steam.accounts.length} 个`);
    } else {
      const available = data.steam.webSessions.filter((session) => {
        if (!session.steamId) return false;
        const profile = perfectProfiles[session.steamId];
        return perfectAvailability(profile, data.perfectUnavailableAccountIds?.includes(session.steamId)) === "ready";
      }).length;
      setMessage(`完美账号 ${data.steam.webSessions.length} 个，${available} 个可快速切换`);
    }
  }

  const activeContent = activeApp === "oopz"
    ? activeFeature === "overview" ? overview : activeFeature === "autoSign" ? autoSign : switcher
    : activeApp === "steam"
      ? activeFeature === "overview" ? steamOverview : steamSwitcher
      : activeFeature === "overview" ? perfectOverview : perfectSwitcher;
  const activeAppName = activeApp === "oopz" ? "OOPZ" : activeApp === "steam" ? "Steam" : "完美对战平台";
  const activeFeatureName = activeFeature === "overview" ? "概览" : activeFeature === "autoSign" ? "自动签到" : "账号切换";

  if (startupPhase !== "ready") {
    return (
      <main className="shell startup-shell" aria-busy={startupPhase === "loading"}>
        <header className="window-titlebar" data-tauri-drag-region onMouseDown={startWindowDrag} onDoubleClick={toggleMaximizeWindow}>
          <div className="window-brand" data-tauri-drag-region>
            <img src="/nea-brand-dark.png" alt="NEA - Not Enough Accounts" draggable={false} data-tauri-drag-region />
          </div>
          <div className="window-controls" onDoubleClick={(event) => event.stopPropagation()}>
            <button onClick={minimizeWindow} aria-label="最小化" title="最小化"><Minus size={15} /></button>
            <button onClick={toggleMaximizeWindow} aria-label="最大化或还原" title="最大化或还原"><Square size={13} /></button>
            <button className="window-close" onClick={closeWindow} aria-label="隐藏到托盘" title="隐藏到托盘"><X size={16} /></button>
          </div>
        </header>
        <section className="startup-state" role={startupPhase === "error" ? "alert" : "status"}>
          <div className="startup-state-card">
            {startupPhase === "loading" && <div className="spinner" aria-hidden="true" />}
            <strong>{startupPhase === "loading" ? "正在加载账号" : "启动未完成"}</strong>
            <span>{startupPhase === "loading" ? "正在恢复数据并准备账号菜单…" : startupError}</span>
            {startupPhase === "error" && <small>请从托盘退出 NEA 后重新打开；原账号数据不会被空配置覆盖。</small>}
          </div>
        </section>
      </main>
    );
  }

  return (
    <main className="shell">
      <header className="window-titlebar" aria-hidden={activeDialogKey ? true : undefined} data-tauri-drag-region onMouseDown={startWindowDrag} onDoubleClick={toggleMaximizeWindow}>
        <div className="window-brand" data-tauri-drag-region>
          <img src="/nea-brand-dark.png" alt="NEA - Not Enough Accounts" draggable={false} data-tauri-drag-region />
        </div>
        <div className="window-controls" onDoubleClick={(event) => event.stopPropagation()}>
          <button className="window-theme" onClick={() => setDarkMode((current) => !current)} aria-label={darkMode ? "切换到浅色模式" : "切换到暗黑模式"} title={darkMode ? "浅色模式" : "暗黑模式"}>{darkMode ? <Sun size={15} /> : <Moon size={15} />}</button>
          <button className="window-update" onClick={() => handleAction(checkForUpdates)} disabled={busy || updateActive} aria-label="检查更新" title={updateStatus?.message || "检查更新"}><RefreshCw className={updateActive ? "spin-icon" : ""} size={15} /></button>
          <button onClick={minimizeWindow} onDoubleClick={(event) => event.stopPropagation()} aria-label="最小化" title="最小化"><Minus size={15} /></button>
          <button onClick={toggleMaximizeWindow} onDoubleClick={(event) => event.stopPropagation()} aria-label="最大化或还原" title="最大化或还原"><Square size={13} /></button>
          <button className="window-close" onClick={closeWindow} onDoubleClick={(event) => event.stopPropagation()} aria-label="隐藏到托盘" title="隐藏到托盘"><X size={16} /></button>
        </div>
      </header>

      <div className="app-layout" aria-hidden={activeDialogKey ? true : undefined}>
        <aside className="app-rail">
          <nav className="app-list" aria-label="软件切换">
            <button data-active={activeApp === "oopz"} aria-current={activeApp === "oopz" ? "page" : undefined} onClick={() => selectApp("oopz")} aria-label="切换到 OOPZ" title="OOPZ"><img className="app-icon-image" src="/oopz-icon.png" alt="" /></button>
            <button data-active={activeApp === "steam"} aria-current={activeApp === "steam" ? "page" : undefined} onClick={() => selectApp("steam")} aria-label="切换到 Steam" title="Steam"><img className="app-icon-image" src="/steam-icon.svg" alt="" /></button>
            <button data-active={activeApp === "perfect"} aria-current={activeApp === "perfect" ? "page" : undefined} onClick={() => selectApp("perfect")} aria-label="切换到完美对战平台" title="完美对战平台"><img className="app-icon-image" src="/perfect-arena-icon.png" alt="" /></button>
          </nav>
          <button className="global-share-button" onClick={openShareCenter} aria-label="账号分享" title="账号分享"><Share2 size={20} strokeWidth={1.9} /></button>
        </aside>
        <aside className="sidebar auto-hide-scrollbar" onScroll={showScrollbarWhileScrolling}>
          <div className="sidebar-app-name">{activeAppName}</div>
          <nav className="feature-list">
            <button data-active={activeFeature === "overview"} aria-current={activeFeature === "overview" ? "page" : undefined} onClick={() => setActiveFeature("overview")}><LayoutDashboard size={17} strokeWidth={2} aria-hidden="true" /><strong>概览</strong></button>
            <button data-active={activeFeature === "switcher"} aria-current={activeFeature === "switcher" ? "page" : undefined} onClick={() => setActiveFeature("switcher")}><UsersRound size={17} strokeWidth={2} aria-hidden="true" /><strong>账号切换</strong></button>
            {activeApp === "oopz" && <button data-active={activeFeature === "autoSign"} aria-current={activeFeature === "autoSign" ? "page" : undefined} onClick={() => setActiveFeature("autoSign")}><CalendarCheck size={17} strokeWidth={2} aria-hidden="true" /><strong>自动签到</strong></button>}
          </nav>
        </aside>

        <section className="workspace auto-hide-scrollbar" data-contained-scroll={activeApp === "perfect" && activeFeature === "switcher" || undefined} onScroll={showScrollbarWhileScrolling}>
          <header className="topbar">
            <h2>{activeAppName} · {activeFeatureName}</h2>
            <div className="status" data-busy={busy} role="status" aria-live="polite" aria-atomic="true" title={message}>{busy && <span className="spinner" />}<span>{message}</span></div>
          </header>
          <nav className="mobile-feature-tabs" aria-label={`${activeAppName} 功能`}>
            <button data-active={activeFeature === "overview"} aria-current={activeFeature === "overview" ? "page" : undefined} onClick={() => setActiveFeature("overview")}><LayoutDashboard size={15} aria-hidden="true" />概览</button>
            <button data-active={activeFeature === "switcher"} aria-current={activeFeature === "switcher" ? "page" : undefined} onClick={() => setActiveFeature("switcher")}><UsersRound size={15} aria-hidden="true" />账号切换</button>
            {activeApp === "oopz" && <button data-active={activeFeature === "autoSign"} aria-current={activeFeature === "autoSign" ? "page" : undefined} onClick={() => setActiveFeature("autoSign")}><CalendarCheck size={15} aria-hidden="true" />自动签到</button>}
          </nav>
          {activeContent}
        </section>
      </div>
      {showShareCenter && (
        <div className="confirm-backdrop share-center-backdrop" onMouseDown={() => !wormholeActive && !busy && setShowShareCenter(false)}>
          <div className="share-center" role="dialog" aria-modal="true" aria-labelledby="share-center-title" tabIndex={-1} onMouseDown={(event) => event.stopPropagation()}>
            <header className="share-center-header">
              <div><h2 id="share-center-title">账号分享</h2><p>选择账号和登录方式，再生成分享码或导出分享包。</p></div>
              <button className="icon-button" onClick={() => setShowShareCenter(false)} disabled={wormholeActive || busy} aria-label="关闭"><X size={17} /></button>
            </header>
            <div className="share-center-toolbar">
              <button onClick={selectAllShareableAccounts} disabled={wormholeActive || busy}>全选可分享账号</button>
              <button onClick={() => { setShareSelection({ oopzAccountIds: [], steamAccounts: [] }); setShareSelectionNotice(""); }} disabled={wormholeActive || busy}>清空</button>
              {shareSelectionNotice && <span className="share-selection-notice" role="status">{shareSelectionNotice}</span>}
              <span className="share-selection-count">已选 {selectedShareCount} 个账号</span>
            </div>
            <div className="share-tree auto-hide-scrollbar" onScroll={showScrollbarWhileScrolling}>
              <section className="share-tree-platform">
                <label className="share-tree-parent"><input type="checkbox" ref={(element) => oopzShareCheckboxRef(element, shareableOopzIds)} checked={oopzShareSelectionState(shareableOopzIds).all} onChange={() => toggleOopzShareBranch(shareableOopzIds)} disabled={wormholeActive || busy || shareableOopzAccounts.length === 0} /><img src="/oopz-icon.png" alt="" /><strong>OOPZ</strong><span>{shareableOopzAccounts.length} 个可分享</span></label>
                <div className="share-tree-children">
                  {shareableOopzAccounts.length === 0 && <div className="share-tree-empty">暂无可分享登录态</div>}
                  {shareableOopzAccounts.map((account) => <label key={account.id}><input type="checkbox" checked={shareSelection.oopzAccountIds.includes(account.id)} onChange={(event) => toggleOopzShareItem(account.id, event.target.checked)} disabled={wormholeActive || busy} /><span><strong>{account.displayName}</strong><small>{accountLabel(account)}</small></span></label>)}
                </div>
              </section>
              <section className="share-tree-platform">
                <div className="share-tree-parent"><img src="/steam-icon.svg" alt="" /><strong>Steam 分享</strong><span>{shareableSteamIdentities.length} 个账号</span></div>
                <div className="share-tree-children">
                  {shareableSteamIdentities.length === 0 && <div className="share-tree-empty">暂无可分享账号</div>}
                  {shareableSteamIdentities.length > 0 && <div className="share-steam-column-head"><span>账号</span><label className="share-steam-capability"><input type="checkbox" ref={(element) => steamCapabilityCheckboxRef(element, "webLogin")} checked={steamCapabilitySelectionState("webLogin").all} onChange={() => toggleSteamCapabilityBranch("webLogin", !steamCapabilitySelectionState("webLogin").all)} disabled={wormholeActive || busy || !shareableSteamIdentities.some((identity) => identity.capabilities.webLogin)} /><span>网页态</span></label><label className="share-steam-capability"><input type="checkbox" ref={(element) => steamCapabilityCheckboxRef(element, "credential")} checked={steamCapabilitySelectionState("credential").all} onChange={() => toggleSteamCapabilityBranch("credential", !steamCapabilitySelectionState("credential").all)} disabled={wormholeActive || busy || !shareableSteamIdentities.some((identity) => identity.capabilities.credential)} /><span>账密</span></label></div>}
                  {shareableSteamIdentities.map((identity) => {
                    const steamId = identity.steamId || "";
                    const choice = selectedSteamShareChoice(steamId);
                    const perfectSelected = Boolean(choice?.perfect);
                    return <div className="share-steam-account-row" key={identity.id}>
                      <span className="share-steam-account"><strong>{identity.note || identity.displayName}</strong><small>{[identity.accountName, steamId].filter(Boolean).join(" · ")}</small></span>
                      <label className="share-steam-capability"><input type="checkbox" checked={Boolean(choice?.webLogin || perfectSelected)} onChange={(event) => setSteamShareCapability(steamId, "webLogin", event.target.checked)} disabled={wormholeActive || busy || !identity.capabilities.webLogin || perfectSelected} /><span>网页态</span></label>
                      <label className="share-steam-capability"><input type="checkbox" checked={Boolean(choice?.credential)} onChange={(event) => setSteamShareCapability(steamId, "credential", event.target.checked)} disabled={wormholeActive || busy || !identity.capabilities.credential} /><span>账密</span></label>
                    </div>;
                  })}
                </div>
              </section>
              <section className="share-tree-platform">
                <div className="share-tree-parent share-tree-perfect-parent">
                  <label className="share-tree-parent-select"><input type="checkbox" ref={(element) => perfectShareCheckboxRef(element, selectablePerfectSteamIds)} checked={perfectShareSelectionState(selectablePerfectSteamIds).all} onChange={() => togglePerfectShareBranch(!perfectShareSelectionState(selectablePerfectSteamIds).all)} disabled={wormholeActive || busy || selectablePerfectIdentities.length === 0} /><img src="/perfect-arena-icon.png" alt="" /><strong>完美对战平台</strong></label>
                  <label className="share-available-filter"><input type="checkbox" checked={sharePerfectAvailableOnly} onChange={(event) => setPerfectShareAvailableOnly(event.target.checked)} disabled={wormholeActive || busy} />仅选择可用账号</label>
                </div>
                <div className="share-tree-children">
                  {perfectShareIdentities.length === 0 && <div className="share-tree-empty">暂无可分享账号</div>}
                  {perfectShareIdentities.map((identity) => {
                    const steamId = identity.steamId || "";
                    const profile = perfectProfiles[steamId];
                    const unavailable = Boolean(data.perfectUnavailableAccountIds?.includes(steamId));
                    const highRisk = Boolean(profile?.highRisk || profile?.reputationRequiresVerification);
                    const reputation = unavailable ? "不可用" : highRisk ? "高危" : profile?.reputationLevel || "待检测";
                    const disabledByFilter = sharePerfectAvailableOnly && !isPerfectShareUsable(identity);
                    const choice = selectedSteamShareChoice(steamId);
                    const selected = Boolean(choice?.perfect);
                    return <label key={identity.id} data-unavailable={unavailable || highRisk || undefined}><input type="checkbox" checked={selected} onChange={(event) => setSteamShareCapability(steamId, "perfect", event.target.checked)} disabled={wormholeActive || busy || unavailable || disabledByFilter} /><span><strong>{profile?.nickname || identity.note || identity.displayName}</strong><small>{steamId}</small><span className="share-perfect-meta"><b>等级分 {perfectScoreLabel(profile?.score)}</b><b>身份 {profile?.playerIdentity || "待检测"}</b><b data-danger={unavailable || highRisk || undefined}>信誉 {reputation}</b><b>网页态</b>{selected && choice?.credential && <b>账密</b>}</span></span></label>;
                  })}
                </div>
              </section>
            </div>
            <div className="share-center-transfer">
              <div className="quick-transfer-row">
                <button className="primary" onClick={() => void startQuickShare()} disabled={busy || wormholeActive || selectedShareCount === 0}>生成分享码</button>
                {quickCode && <code className="quick-code">{quickCode}</code>}
                {quickCode && quickPackageBytes !== null && <span className="quick-package-size">文件 {formatFileSize(quickPackageBytes)}</span>}
                {quickCode && <button onClick={() => copyText(quickCode)}>复制代码</button>}
                {wormholeActive && wormholeStatus?.state !== "committing" && <button onClick={() => void cancelQuickShare()} disabled={wormholeStatus?.state === "cancelling"}>取消</button>}
              </div>
              <div className="quick-transfer-row">
                <input value={receiveCode} onChange={(event) => setReceiveCode(event.target.value)} placeholder="输入对方分享码" disabled={busy || wormholeActive} />
                <button onClick={() => handleAction(quickImport)} disabled={busy || wormholeActive || !receiveCode.trim()}>接收并导入</button>
              </div>
              <div className="quick-transfer-row share-file-transfer-row">
                <button onClick={() => handleAction(exportSharePackageFile)} disabled={busy || wormholeActive || selectedShareCount === 0}>导出分享包</button>
                <button onClick={() => handleAction(importSharePackageFile)} disabled={busy || wormholeActive}>导入分享包</button>
              </div>
              {wormholeStatus && <div className="quick-transfer-status" data-state={wormholeStatus.state} role="status" aria-live="polite" aria-atomic="true">{wormholeStatus.message}</div>}
              {wormholeStatus?.total && wormholeStatus.transferred !== undefined && <progress value={wormholeStatus.transferred} max={wormholeStatus.total} />}
            </div>
          </div>
        </div>
      )}
      {pendingOopzOperation && (
        <div className="confirm-backdrop" onMouseDown={() => !busy && setPendingOopzOperation(null)}>
          <div className="confirm-dialog" role="alertdialog" aria-modal="true" aria-labelledby="oopz-operation-title" aria-describedby="oopz-operation-description" tabIndex={-1} onMouseDown={(event) => event.stopPropagation()}>
            <p id="oopz-operation-title"><strong>{pendingOopzOperation.kind === "switch" ? `切换到“${pendingOopzOperation.account.displayName}”？` : "恢复最近一次 OOPZ 备份？"}</strong></p>
            <small id="oopz-operation-description" className="confirm-description">{pendingOopzOperation.kind === "switch" ? "NEA 会先备份当前登录状态，再关闭并重启 OOPZ。若切换失败，将自动尝试恢复。" : "NEA 将关闭 OOPZ，并恢复最近一次切换前保存的账号状态。"}</small>
            <div className="confirm-actions"><button className="primary" onClick={confirmOopzOperation} disabled={busy}>{pendingOopzOperation.kind === "switch" ? "确认切换" : "确认恢复"}</button><button onClick={() => setPendingOopzOperation(null)} disabled={busy} autoFocus>取消</button></div>
          </div>
        </div>
      )}
      {pendingDeleteAccount && (
        <div className="confirm-backdrop" onMouseDown={() => !busy && setPendingDeleteAccount(null)}>
          <div className="confirm-dialog" role="alertdialog" aria-modal="true" aria-labelledby="delete-confirm-title" aria-describedby="delete-confirm-description" tabIndex={-1} onMouseDown={(event) => event.stopPropagation()}>
            <p id="delete-confirm-title"><strong>删除“{pendingDeleteAccount.displayName}”及其本地登录态？</strong></p>
            <small id="delete-confirm-description" className="confirm-description">将删除 NEA 保存的 OOPZ 账号快照和凭据；不会删除 OOPZ 程序。</small>
            <div className="confirm-actions">
              <button className="primary danger-confirm" onClick={confirmDeleteSelected} disabled={busy}>删除账号及登录态</button>
              <button onClick={() => setPendingDeleteAccount(null)} disabled={busy} autoFocus>取消</button>
            </div>
          </div>
        </div>
      )}
      {pendingDeleteSteamWebSession && (
        <div className="confirm-backdrop" onMouseDown={() => !busy && setPendingDeleteSteamWebSession(null)}>
          <div className="confirm-dialog" role="alertdialog" aria-modal="true" aria-labelledby="steam-web-delete-confirm-title" aria-describedby="steam-web-delete-confirm-description" tabIndex={-1} onMouseDown={(event) => event.stopPropagation()}>
            <p id="steam-web-delete-confirm-title"><strong>删除“{pendingDeleteSteamWebSession.displayName}”的网页登录？</strong></p>
            <small id="steam-web-delete-confirm-description" className="confirm-description">将删除 WebView2 网页会话并影响完美平台入口；已保存的 Steam 账号密码会保留。</small>
            <div className="confirm-actions"><button className="primary danger-confirm" onClick={() => handleAction(() => deleteSteamWebSession(pendingDeleteSteamWebSession))} disabled={busy}>删除网页登录</button><button onClick={() => setPendingDeleteSteamWebSession(null)} disabled={busy} autoFocus>取消</button></div>
          </div>
        </div>
      )}
      {pendingDeleteSteamCredential && (
        <div className="confirm-backdrop" onMouseDown={() => !busy && setPendingDeleteSteamCredential(null)}>
          <div className="confirm-dialog" role="alertdialog" aria-modal="true" aria-labelledby="steam-credential-delete-title" aria-describedby="steam-credential-delete-description" tabIndex={-1} onMouseDown={(event) => event.stopPropagation()}>
            <p id="steam-credential-delete-title"><strong>清除“{pendingDeleteSteamCredential.displayName}”保存的 Steam 账密？</strong></p>
            <small id="steam-credential-delete-description" className="confirm-description">网页登录会保留，但 NEA 将不能再用账密登录 Steam 客户端。之后可通过文本导入重新保存新密码。</small>
            <div className="confirm-actions"><button className="primary danger-confirm" onClick={() => handleAction(() => deleteSteamCredential(pendingDeleteSteamCredential))} disabled={busy}>清除已保存账密</button><button onClick={() => setPendingDeleteSteamCredential(null)} disabled={busy} autoFocus>取消</button></div>
          </div>
        </div>
      )}
      {showSteamTextImport && (
        <div className="confirm-backdrop" onMouseDown={closeSteamTextImportDialog}>
          <div className="confirm-dialog steam-text-import-dialog" role="dialog" aria-modal="true" aria-labelledby="steam-text-import-title" tabIndex={-1} onMouseDown={(event) => event.stopPropagation()}>
            <p id="steam-text-import-title">从文本导入 Steam 网页账号</p>
            <textarea value={steamTextImportDraft} onChange={(event) => setSteamImportDraftState(event.target.value)} placeholder={"账号 密码\n账号 密码"} autoFocus spellCheck={false} aria-invalid={Boolean(steamImportError)} aria-describedby="steam-text-import-hint steam-text-import-feedback" />
            <small className="steam-import-parallel-hint">一次最多导入 100 个账号，同时打开最多 4 个独立登录窗口；优先解析 Steam64，仅跳过已有有效网页登录态的账号，只有客户端态时会继续补齐网页能力。</small>
            <span id="steam-text-import-hint" className="sr-only">每行一个账号，账号和密码之间至少包含一个空格。</span>
            <div id="steam-text-import-feedback">
              {steamImportError && <div className="steam-import-inline-error" role="alert">{steamImportError}</div>}
              {steamImportPreview && <div className="steam-import-duplicate-notice"><strong>导入前检查发现需要确认的项目</strong>{steamImportPreview.existingAccounts.length > 0 && <span>已有有效网页登录：{steamImportPreview.existingAccounts.join("、")}</span>}{steamImportPreview.duplicateInputAccounts.length > 0 && <span>重复输入：{steamImportPreview.duplicateInputAccounts.join("、")}</span>}<small>已有网页登录的账号仍会补存缺失账密；信息完整的账号会跳过。重复输入只处理一次。</small></div>}
            </div>
            <div className="confirm-actions"><button className="primary" onClick={() => handleAction(steamImportPreview ? confirmSteamWebImport : prepareSteamWebImport)} disabled={busy || !steamTextImportDraft.trim()}>{steamImportPreview ? "确认并继续导入" : "检查并导入"}</button><button onClick={closeSteamTextImportDialog} disabled={busy}>取消</button></div>
          </div>
        </div>
      )}
    </main>
  );
}

export default App;

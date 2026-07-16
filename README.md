<div align="center">
  <img src="src-tauri/icons/icon.png" width="152" alt="NEA 图标">

# NEA

**Not Enough Accounts · Windows 本地多平台账号工作台**

[![Release](https://img.shields.io/badge/release-v1.2.7-2563eb)](https://github.com/M4rkzzz/NEA/releases/latest)
[![Windows](https://img.shields.io/badge/Windows-x64-0078d4?logo=windows)](#运行环境)
[![Tauri](https://img.shields.io/badge/Tauri-2-24c8db?logo=tauri)](https://tauri.app/)

[下载最新版](https://github.com/M4rkzzz/NEA/releases/latest) · [查看 Releases](https://github.com/M4rkzzz/NEA/releases) · [反馈问题](https://github.com/M4rkzzz/NEA/issues)
</div>

NEA 是面向 Windows 的本地多软件账号切换器，把 OOPZ、Steam 与完美世界竞技平台的账号资料、登录能力和常用入口收拢到一个桌面应用中。数据默认留在当前 Windows 用户目录，不依赖 NEA 自建云端服务。

> NEA 由 OOPZ+ 演进而来。升级时会兼容迁移旧版配置、账号快照、凭据、自启动项与 `.oopz+` 登录态包。

## 支持的平台

| 平台 | 主要能力 |
| --- | --- |
| OOPZ | 本地账号快照、一键切号、托盘与头像浮层、导入导出、加密通道快捷分享 |
| Steam | SteamID64 统一身份、网页登录、保存账密登录客户端、Steam Guard 状态识别、托盘切号 |
| 完美世界竞技平台 | 账号资料与筛选、网页授权同步、使用关联 Steam 账密打开客户端 |

## 主要能力

- 统一账号视图：按稳定身份合并不同来源的数据，保留网页能力、客户端账密和平台资料。
- Steam 稳定切号：通过已保存账密启动 Steam，持续核验进程与实际 `ActiveUser`，慢速网络下也会显示进度并进行失败恢复。
- Steam 网页批量导入：导入前后按 SteamID64 查重，识别密码错误、手机令牌和电子邮件验证，单个错误不会阻塞后续账号。
- 原生最近账号隔离：可将 NEA 账密登录的账号从 Steam 原生“切换账号”最近列表中安全移除，避免挤占五个位置。
- OOPZ 多入口切号：支持主界面、系统托盘与贴附浮层，并在切换前保存恢复点。
- 登录态迁移：`.nea` / `.oopz+` 文件和 Magic Wormhole 一次性代码可用于跨设备传递已保存状态。
- 本地维护：会话缓存瘦身、孤立目录回收、事务恢复和旧数据迁移都由应用内完成。

## 1.2.7 重点更新

- Steam 批量网页登录增加导入前预检查、明确取消入口和结构化结果，可分类查看密码错误、令牌、邮件验证、其他失败与未处理账号。
- 修复取消与登录成功同时发生时的竞态；二维码刷新入口出现后仍会立即刷新，不把未加载二维码误判为失败。
- OOPZ 单账号与账号包导入改为原子合并和事务回滚，配置损坏时可从已验证事务备份受控恢复。
- Steam 原生最近账号清理改为单后台任务并增加失败重试，避免线程堆积和瞬时写入失败丢任务。
- 完美平台统一“可用、待检测、不可用”判定，修复互斥筛选导致空列表的问题。
- 分享中心默认不选择任何敏感登录态；密码查看会在超时、失焦或切换页面时自动隐藏。
- OOPZ 切号、恢复和各类删除操作使用统一确认弹窗，明确操作影响并支持完整键盘焦点管理。
- 重新整理最小窗口、窄屏、暗色主题、空状态和结果面板，560×420 下仍可完整操作且不横向溢出。

完整变更与安装包见 [NEA v1.2.7 Release](https://github.com/M4rkzzz/NEA/releases/tag/v1.2.7)。

## 安装与使用

1. 从 [Releases](https://github.com/M4rkzzz/NEA/releases/latest) 下载 `NEA_1.2.7_x64_en-US.msi`。
2. 安装并打开 NEA；首次启动会自动检查旧 OOPZ+ 数据并执行兼容迁移。
3. 在左侧选择平台，根据页面提示识别程序路径、导入账号或登录一次。
4. 后续可从账号列表、托盘菜单或 OOPZ 浮层快速切换。

Steam Guard、手机确认和邮件验证仍由 Steam 官方页面或客户端完成。NEA 不绕过任何二次验证。

## 安全与隐私

请在使用前了解以下边界：

- **Steam 用户名和密码以明文保存在 `%APPDATA%\NEA\config.json`。** 只应在可信的个人 Windows 账户和受控设备上启用此能力，并妥善保护该文件、系统备份与远程访问权限。
- `.nea` 和旧 `.oopz+` 包不包含 NEA 保存的 Steam 密码，但会包含可用于登录的本地或网页状态，仍属于敏感文件，应仅通过可信渠道传递并及时删除。
- Magic Wormhole 快捷分享使用一次性代码和端到端加密通道；中继服务不能读取包内容，但接收方获得的仍是敏感登录状态。
- NEA 不修改平台程序文件，不注入 DLL，不逆向登录协议，也不绕过 Steam Guard。
- NEA 默认只管理当前 Windows 用户的数据；共享 Windows 账户会扩大配置与登录态的可见范围。

## 数据目录

NEA 的稳定数据入口是 `%APPDATA%\NEA`：

```text
NEA/
├─ config.json                     主配置（包含已保存的 Steam 明文账密）
├─ config.json.bak                 主配置原子备份
├─ workspaces/
│  ├─ oopz/                        OOPZ 账号快照与切号备份
│  ├─ steam/web-sessions/          Steam WebView2 必要登录态
│  └─ perfect/avatars/             完美平台头像缓存
├─ runtime/                        Watcher 与更新运行状态
├─ recovery/                       可恢复事务数据
└─ legacy/                         旧目录迁移归档
```

Steam 会话只保留 Cookie、Local Storage 等必要状态。页面缓存、代码缓存、GPU/Shader 缓存和统计文件会在窗口关闭或“整理存储”时清理；升级后目录结构保持兼容，不要求用户手工移动文件。

## 运行环境

- Windows 10 x64 1709 或更高版本；推荐 Windows 10 22H2 / Windows 11。
- 不支持 32 位 Windows。
- 图形界面依赖 Microsoft Edge WebView2 Runtime；MSI 会在缺失时引导安装。
- Steam、OOPZ 和完美世界竞技平台的可用性仍受各自客户端、网络与账号安全策略影响。

## 本地开发

需要 Node.js、pnpm、Rust stable 和 Tauri 2 的 Windows 构建环境。

```powershell
pnpm install
pnpm run dev:app       # 开发运行
pnpm run check:fast    # TypeScript + Rust 增量检查
pnpm run verify:full   # 正式发布前完整验证
pnpm run build:msi     # 构建 Windows MSI
```

安装包输出到 `src-tauri/target/release/bundle/msi/`。生产构建会强制检查并嵌入前端入口，避免生成无法显示界面的安装包。

## 常见问题

### 为什么 Steam 账号显示“需要账密”？

Steam 原生最近登录记录现在只作为名称和当前状态的参考，不再当作可靠登录能力。保存账密后，NEA 才能主动登录客户端；遇到 Steam Guard 时仍需按官方提示确认。

### NEA 会占用 Steam 原生切换账号的五个位置吗？

NEA 会记录通过自身账密登录的账号，并仅在 Steam 完全退出的安全窗口清理其“记住密码”、自动登录、最近标记和排序时间，尽量不占用原生最近列表。Steam 正在运行时不会改写该文件。

### 可以跨电脑迁移吗？

可以。OOPZ 登录态包与 Steam 网页会话支持导入或快捷分享；Steam 客户端账密不会写入导出包，需要在目标电脑单独保存。

### 升级会打乱 `%APPDATA%\NEA` 吗？

不会。`config.json` 保持稳定入口，其余数据按 `workspaces/runtime/recovery/legacy` 分层；迁移由程序执行，并保留旧版兼容路径与恢复数据。

## 许可证

本仓库当前未声明开源许可证。未经作者授权，请勿用于商业分发或二次发布。

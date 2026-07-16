# NEA AI 开发交接

这份文件是后续 AI 接手项目时的第一入口。目标是快速形成正确心智模型，避免重做已经验证失败的路线或把开发流程复杂化。

## 1. 开始工作

1. 先运行 `git status --short`，现有修改和未跟踪文件默认属于用户，不得清理、覆盖或回退。
2. 阅读本文件、`README.md`，再按任务阅读对应模块；不要为了小改动全面重构。
3. 优先用代码、日志和命令行定位问题。本项目当前禁止使用 Computer Use，除非用户以后明确重新授权。
4. 保持快速迭代：做最小完整修改，验证与风险相称，不云端编译，不因普通开发任务构建 MSI。

## 2. 项目定位与技术栈

NEA（Not Enough Accounts）是 Windows 本地多平台账号切换与登录状态管理工具，当前支持 OOPZ、Steam 和完美世界竞技平台。

- 桌面框架：Tauri 2。
- 前端：React 19、TypeScript、Vite，主要入口为 `src/App.tsx`，样式集中在 `src/App.css`。
- 后端：Rust，主要业务和 Tauri command 集中在 `src-tauri/src/lib.rs`。
- Steam 客户端适配：`src-tauri/src/steam.rs`。
- 完美平台适配：`src-tauri/src/perfect_arena.rs`。
- 应用配置：`src-tauri/tauri.conf.json`；权限：`src-tauri/capabilities/default.json`。
- 正式项目名只有 NEA；OOPZ+ 旧名称仅保留在数据、安装和包格式兼容代码中。

`src-tauri/src/lib.rs` 较大。修改前先用 `rg` 定位 command、数据结构和对应测试，不要从头通读或为拆文件而拆文件。

## 3. 不可随意改变的产品决策

### Steam

- Steam 客户端原生登录态已经被判定为不可靠，不再作为切号能力。客户端默认使用用户保存的账号密码登录。
- Steam 用户名和密码按用户明确要求明文写入 `%APPDATA%\NEA\config.json`。这是已知风险和当前决策，不要擅自迁移到加密、Keyring 或令牌方案。
- `loginusers.vdf` 只作元数据参考，并在 Steam 完全退出时安全清理 NEA 管理账号的最近标记，尽量不占原生切换账号的五个位置。
- Steam 账号只要具备网页登录态或已保存账密任一能力即可保留；只有原生客户端元数据、没有这两种能力的条目应清理。完美资料本身不能留下空 Steam 账号。
- 导入账密时先查询 SteamID64 再查重。SteamID64 重复不能阻止补齐缺失能力：缺账密就保存账密，缺网页态就导入网页态；已有账密不得被重复导入覆盖。
- Steam 登录账号名与 Steam 社区显示名称是两个字段，列表主名称应优先显示社区名称，不能把登录账号名冒充为社区名称。
- 网页导入必须分别识别密码错误、手机令牌和邮件验证；单账号失败不能阻塞后续账号。
- 二维码出现刷新按钮时立即点击刷新并继续等待，不能用固定 12 秒轮询，也不能仅因二维码未加载就判定失败。
- Steam Guard 和其他验证始终走官方页面/客户端；不注入 DLL，不绕过验证，不实现 EYA/JWT/ConnectCache 令牌登录。

### 分享

- 分享支持 OOPZ、Steam 网页登录态和完美平台附加数据。
- “只勾选完美仍携带完整 Steam 网页 Cookie”是明确保留的特性，不要把完美改成独立分享权限。UI 应明确说明这种组合能力。
- 分享包不包含 Steam 客户端密码，但包含敏感登录态。手动包扩展名为 `.nea-share`，落盘后没有额外加密。
- 快捷分享使用 Magic Wormhole，保留设备直连；需要中继时只使用免费的 Winden / Least Authority 公共中继。不要重新加入旧的慢中继竞速。
- 接收端只安装必要 Steam Cookie，并校验域、SteamID64、包清单、数量、大小、路径穿越和完美 SQLite 数据库。
- 导入必须先暂存再提交，支持取消、逆序回滚和启动恢复；提交阶段不能暴露可取消入口。
- 恢复记录位于 `%APPDATA%\NEA\recovery`，只记录路径和受影响 ID，不得写入 Steam 密码。

### 数据与并发

- 当前数据根目录是 `%APPDATA%\NEA`，稳定入口为 `config.json`，其余内容按 `workspaces`、`runtime`、`recovery`、`legacy` 分层。
- 不要更改稳定路径来“整理目录”；迁移必须兼容旧版并可恢复。
- Steam 网页导入、批量导入、能力补全和存储整理之间已有互斥保护，不得绕过。
- 配置更新必须基于最新内存数据提交，禁止先克隆、长时间操作后覆盖整个配置，这会重新引入并行导入丢失更新。
- WebView 会话瘦身只能删除缓存、代码缓存、GPU/Shader 缓存和统计数据，必须保留 Cookie、Local Storage 等登录状态。

## 4. 验证纪律

开发验证分三档，不要在普通迭代中反复跑 Release 全量验证：

```powershell
pnpm run check:fast      # 紧密编辑循环：TypeScript + cargo check，不跑测试
pnpm run verify:dev      # 功能完成/交付前：TypeScript + 5 项关键 Rust 回归
pnpm run verify:release  # 仅正式发布：构建、格式、74 项稳定测试、Clippy
```

五项开发冒烟测试统一使用 `dev_smoke_` 前缀，一次启动测试进程，覆盖：

1. 并发 Steam 会话提交不丢更新。
2. 重复 SteamID 仍补缺失账密且不覆盖旧密码。
3. 只有客户端元数据时仍允许补网页登录态。
4. `.nea-share` 写入、解析和原子覆盖。
5. 分享文件提交失败后的逆序回滚。

两项依赖外部环境的测试默认 `ignored`，不纳入稳定的 Release 命令：

```powershell
# 公网 Magic Wormhole/Winden，仅在需要验证公共链路时运行
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib tests::magic_wormhole_public_roundtrip -- --exact --ignored

# 仅在本机安装完美平台并配置 NEA_PERFECT_TEST_IDS 后运行
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib perfect_arena::tests::queries_multiple_profiles_with_official_client_signature -- --exact --ignored
```

测试总数变化时同步更新 README 和本文件中的数量。修复高风险回归时，优先增加一个精确测试；只有它属于上述五类核心门禁时才加入 `dev_smoke_`，否则留在 Release 套件。

## 5. 本地发布

禁止通过 GitHub Actions 或其他云端环境编译 MSI。发布流程必须保持简单：

1. 同步更新 `package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json` 和 README 版本。
2. 添加 `.github/releases/vX.Y.Z.md`。
3. 本地运行 `pnpm install --frozen-lockfile` 和 `pnpm run verify:release`。
4. 本地运行 `pnpm run build:msi`。
5. 从 `src-tauri/target/release/bundle/msi/` 取出本地 MSI，计算 SHA-256 后上传或替换 GitHub Release 资产。

不要因用户说“发布”就自行设计 CI/CD、矩阵构建、签名服务或额外制品流水线。具体命令见 `docs/GITHUB_SETUP.md`。

## 6. 安全和已知边界

- `config.json` 含明文 Steam 密码；任何日志、错误、测试快照、恢复日志和分享包都不得输出或复制密码。
- `.nea-share`、OOPZ `.nea` 和旧 `.oopz+` 都应按敏感文件处理。
- 当前有一个极窄崩溃窗口：混合分享包在 OOPZ 已提交、完成标记尚未落盘时被强杀，启动恢复可能保留 OOPZ 而回滚 Steam/完美。正常失败、取消和常规恢复路径不受影响。若修复，必须避免扩大事务复杂度或破坏字段级并发保护。
- 网络、Steam/完美页面结构和第三方客户端更新属于外部不稳定面。改动自动化选择器时要保留多信号识别和清晰的超时/取消状态。

## 7. 交付前检查

- 改动是否严格对应用户请求，没有顺手扩大范围。
- 是否保留了脏工作区中的无关文件。
- 是否把账号登录名、社区显示名、SteamID64 混为一谈。
- 是否在重复账号路径补齐缺失能力，而不是过早 `continue`。
- 是否会覆盖并发产生的新配置。
- 是否泄露密码、Cookie、令牌或本地路径到日志/包/恢复记录。
- 是否只运行了与当前阶段匹配的验证档位。
- 若是 Release，是否使用本地 MSI，而非触发云端编译。

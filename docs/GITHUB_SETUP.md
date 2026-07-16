# NEA GitHub 发布资料

本项目的正式名称、仓库名和发布资产统一使用 `NEA`。

## 仓库信息

- 仓库：`M4rkzzz/NEA`
- 简介：`NEA（Not Enough Accounts）— Windows 本地多平台账号切换与登录状态管理工具，支持 OOPZ、Steam 和完美世界竞技平台。`
- Release：`https://github.com/M4rkzzz/NEA/releases/latest`

推荐 Topics：

```text
nea
not-enough-accounts
account-switcher
tauri
react
typescript
rust
windows
desktop-app
steam
oopz
perfect-world-arena
```

## 发布约定

- Tag 使用 `v<major>.<minor>.<patch>`，例如 `v1.2.7`。
- Release 标题使用 `NEA <tag>`，例如 `NEA v1.2.7`。
- 安装包使用 `NEA_<version>_x64_en-US.msi`。
- Release 说明保存在 `.github/releases/<tag>.md`。
- 正式发布前同步更新 `package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json` 和 README 版本。

## 本地发布检查

```powershell
pnpm install --frozen-lockfile
pnpm run verify:full
pnpm run build:msi
```

安装包生成于：

```text
src-tauri/target/release/bundle/msi/
```

## GitHub Actions

推送 `v*` Tag 会触发 `.github/workflows/release.yml`：

1. 安装 pnpm、Node.js 与 Rust。
2. 运行完整验证。
3. 构建 Windows MSI。
4. 使用同名 Release 说明发布安装包。

发布前必须先创建对应的 `.github/releases/<tag>.md`，否则工作流会主动失败，避免发布错误版本的说明。

## 手动发布

在 GitHub CLI 已登录的环境中：

```powershell
$tag = "v1.2.7"
$version = $tag.TrimStart("v")
gh release create $tag "src-tauri/target/release/bundle/msi/NEA_${version}_x64_en-US.msi" --repo M4rkzzz/NEA --title "NEA $tag" --notes-file ".github/releases/$tag.md"
```

旧版安装包名、数据目录、凭据服务、快捷方式和 `.oopz+` 文件只保留在兼容迁移代码中，不再作为当前发布名称使用。

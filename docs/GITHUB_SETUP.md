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

- Tag 使用 `v<major>.<minor>.<patch>`，例如 `v1.3.0`。
- Release 标题使用 `NEA <tag>`，例如 `NEA v1.3.0`。
- 安装包使用 `NEA_<version>_x64_en-US.msi`。
- Release 说明保存在 `.github/releases/<tag>.md`。
- 正式发布前同步更新 `package.json`、`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock` 中的 `nea` 根包、`src-tauri/tauri.conf.json` 和 README 版本。

## 本地发布检查

```powershell
pnpm install --frozen-lockfile
pnpm run verify:release
pnpm run build:msi
```

安装包生成于：

```text
src-tauri/target/release/bundle/msi/
```

## 本地构建与发布

本项目不使用 GitHub Actions 云端编译。MSI 必须在本地完成 `verify:release` 和构建，再把已经生成的安装包上传到 GitHub Release。发布前先创建对应的 `.github/releases/<tag>.md`。

先提交并推送代码，再创建和推送 Tag。随后在 GitHub CLI 已登录的环境中只上传本地 MSI：

```powershell
$tag = "v1.3.0"
$version = $tag.TrimStart("v")
$msi = Get-Item "src-tauri/target/release/bundle/msi/NEA_${version}_x64_en-US.msi"
gh release create $tag $msi.FullName --repo M4rkzzz/NEA --verify-tag --title "NEA $tag" --notes-file ".github/releases/$tag.md"
```

替换已有 Release 中的本地 MSI：

```powershell
gh release upload $tag $msi.FullName --repo M4rkzzz/NEA --clobber
```

NEA 只提供完整 MSI，不发布增量包、差分包或独立 `.sha256` 文件。应用仍会读取 GitHub Release API 为 MSI 提供的摘要，并在安装前校验完整性。

旧版安装包名、数据目录、凭据服务、快捷方式和 `.oopz+` 文件只保留在兼容迁移代码中，不再作为当前发布名称使用。

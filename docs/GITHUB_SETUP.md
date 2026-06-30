# GitHub 仓库发布资料

本文件用于创建 GitHub 仓库首页、简介、Topics 和首个 Release。

## 推荐仓库名

GitHub 仓库名建议使用：

```text
oopz-plus
```

原因：GitHub 仓库名不建议使用 `+`，用 `oopz-plus` 更稳定，也方便命令行和链接传播。

## 仓库简介

```text
OOPZ+ 是面向 Windows 的 OOPZ 附属增强工具，提供账号快速切换、托盘切号和插件浮层模式。
```

## Website / 主页链接

发布 Release 后可填写：

```text
https://github.com/<你的用户名>/oopz-plus/releases/latest
```

## Topics

建议添加以下 topics：

```text
oopz
oopz-plus
account-switcher
tauri
react
typescript
rust
windows
desktop-app
system-tray
plugin-mode
account-management
```

## 首个 Release

Tag：

```text
v1.0.0
```

Release 标题：

```text
OOPZ+ 1.0.0
```

Release 附件：

```text
src-tauri/target/release/bundle/msi/OOPZ+_1.0.0_x64_en-US.msi
```

Release 内容使用：

```text
.github/releases/v1.0.0.md
```

## 使用 GitHub CLI 发布

需要安装 GitHub 官方 CLI，并登录账号：

```bash
gh auth login
```

创建仓库并推送：

```bash
git init
git add .
git commit -m "发布 OOPZ+ 1.0.0"
gh repo create oopz-plus --public --source . --remote origin --push --description "OOPZ+ 是面向 Windows 的 OOPZ 附属增强工具，提供账号快速切换、托盘切号和插件浮层模式。"
```

设置 topics：

```bash
gh repo edit --add-topic oopz --add-topic oopz-plus --add-topic account-switcher --add-topic tauri --add-topic react --add-topic typescript --add-topic rust --add-topic windows --add-topic desktop-app --add-topic system-tray --add-topic plugin-mode --add-topic account-management
```

创建首个 Release：

```bash
gh release create v1.0.0 "src-tauri/target/release/bundle/msi/OOPZ+_1.0.0_x64_en-US.msi" --title "OOPZ+ 1.0.0" --notes-file ".github/releases/v1.0.0.md"
```

## 重要说明

当前机器没有可用的 GitHub 授权环境变量，且 `gh` 命令不是 GitHub 官方 CLI。需要先安装并登录 GitHub 官方 CLI，或提供可用的 `GITHUB_TOKEN` / `GH_TOKEN` 后才能自动创建远程仓库和 Release。

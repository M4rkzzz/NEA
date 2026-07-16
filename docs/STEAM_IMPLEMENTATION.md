# Steam 客户端实现说明

NEA 当前不把 `loginusers.vdf` 中的原生最近登录记录视为可复用登录能力。该文件只用于读取账号名称、当前状态等元数据，以及在 Steam 完全退出的安全窗口中移除由 NEA 管理的最近账号标记，尽量不占用原生切换账号的五个位置。

客户端切号流程：

1. 请求 `steam.exe -shutdown`，等待 Steam 与 Steam WebHelper 完全退出；无法安全停止时中止切换。
2. 从 NEA 配置读取用户明确保存的 Steam 用户名和密码。
3. 使用 `steam.exe -login <account-name> <password>` 启动客户端。
4. Steam Guard、手机确认和邮件验证继续由 Steam 官方流程处理。
5. 客户端退出后，在安全窗口清理该账号的原生最近列表标记。

Steam 密码按既定产品决策明文保存在 `%APPDATA%\NEA\config.json`。用户在分享中心勾选“账密”时，该账号密码会进入 `.nea-share` 或一次性码的加密传输包；接收端只补充缺失账密，不覆盖已有密码。不要重新引入 EYA token、JWT/ConnectCache 注入、DLL 注入或绕过 Steam Guard 的方案。

NEA 曾参考 [`tuntun1337/opensteameya`](https://github.com/tuntun1337/opensteameya) 的进程生命周期思路，但当前登录实现只使用 Steam 官方命令行账密入口，没有复制其令牌登录机制。

# Steam adapter notes

NEA's Steam process lifecycle and local-login selection behavior were reviewed against
[`tuntun1337/opensteameya`](https://github.com/tuntun1337/opensteameya), an MIT-licensed project.

NEA reimplements only the parts needed to switch accounts already remembered by the local Steam
client:

- request shutdown with `steam.exe -shutdown` and wait for Steam and Steam WebHelper to exit;
- abort if the processes cannot be stopped, preventing a late Steam write from replacing the new
  login state;
- select the local account in `loginusers.vdf`;
- launch with `steam.exe -login <account-name>`.

NEA does not copy opensteameya's EYA token login, JWT encryption, ConnectCache injection, account
export, or Steam Guard behavior. Passwords, tokens, and Steam Guard remain under Steam's control.

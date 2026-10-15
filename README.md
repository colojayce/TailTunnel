# TailTunnel

在 Mac 上管理多条 SSH 本地转发。点一条隧道，输出显示在窗口底部。配置文件的位置在设置里。

配置和密码写在本机应用数据目录的 `tunnels.json`。关掉列表窗口会退出程序，并停掉它拉起的 ssh 子进程。

```bash
npm install
npm run tauri dev
```

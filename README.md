<p align="center">
  <img src="./assets/readme/hero.svg" width="100%" alt="TailTunnel：一个 Mac 窗口里开关多条 SSH 本地转发。左侧是隧道列表，每条显示本地端口到远端的映射，选中后底部显示 ssh 输出。">
</p>

在 Mac 上集中开关多条 SSH 本地转发。每条隧道单独启动或停止，选中后它的 ssh 输出显示在列表下面。

<p align="center">
  <img src="./assets/readme/layout.svg" width="100%" alt="一条隧道就是一次 ssh -L：同一次连接上，本机的 5432 接到远端的 127.0.0.1:5432，6379 接到 10.0.3.4:6379。">
</p>

## 功能

- 一条隧道就是一次 `ssh -L`，同一次连接上可以挂多个本地端口。
- 每条隧道有四种状态：已停止、正在连接、运行中、连接失败。点「运行中」要等 SSH 真正开始监听才算，不是发出命令就算。
- 点列表里的端口号，用浏览器打开 `localhost` 上的这个端口。
- 选中一条隧道，它的 ssh 输出显示在列表下面，可以复制，也可以导出成 JSON。
- 关掉窗口后隧道继续运行，程序留在 Dock 和菜单栏。菜单栏图标旁边是正在运行的条数，点开可以开关每条隧道，或重新打开窗口。
- 退出程序时，这次启动的 SSH 子进程会一起停掉。程序不会在下次打开时按旧进程号去结束别的程序。

SSH 端口不出现在列表里，只在编辑隧道时填写。备注可以不填；填了之后，窗口和菜单栏里显示备注，不填则显示用户和主机。

## 界面

左侧有三页。

- **隧道**是隧道列表和终端。
- **Tailscale** 列出内网里的机器和内网地址，地址可以复制。在线的机器会标出是直连还是经过 DERP 中继，这台电脑标成「本机」。这台 Mac 上如果没有 Tailscale，这一页只说明没有安装，不提供安装。
- **设置**里是配置文件的路径，可以换成别的 JSON 文件。

## 从源码构建

需要本机已安装 Node、Rust 和 OpenSSH，目前只在 macOS 上开发和运行。

```bash
git clone https://github.com/colojayce/TailTunnel.git
cd TailTunnel
npm install
npm run tauri dev
```

打开后先新建一条隧道，填主机、SSH 端口、用户、密码和本地转发，保存后再点启动。

打包：

```bash
npm run tauri build
```

## 配置

配置和密码写在本机的一个 JSON 文件里，默认是：

```text
~/Library/Application Support/com.tailtunnel.app/tunnels.json
```

这个文件包含 SSH 密码，请不要提交到仓库，也不要分享。设置页里可以换成别的路径。

## 技术栈

界面是 Tauri 2 应用：Rust 负责 SSH 子进程、配置文件和 Tailscale 状态，前端是 TypeScript 和 Vite。隧道用的是系统里的 OpenSSH，不是自己实现的 SSH。

## 许可证

[MIT](./LICENSE)

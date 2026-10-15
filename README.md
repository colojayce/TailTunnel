<p align="center">
  <img src="./assets/readme/hero.svg" width="100%" alt="TailTunnel：在一个 Mac 窗口里开关多条 SSH 本地转发，终端贴在列表下面。">
</p>

# TailTunnel

在 Mac 上集中管理多条 SSH 本地转发。每条隧道可单独启动或停止；选中后，其输出显示在窗口底部。

<p align="center">
  <img src="./assets/readme/layout.svg" width="100%" alt="三块功能：隧道显示端口映射，终端显示 ssh 输出，Tailscale 显示 Online 或 Offline。">
</p>

## 它做什么

一条隧道就是一次 `ssh -L`。一条上面可以挂多个本地端口，例如 `3430 → 127.0.0.1:3430`。列表里不显示 SSH 端口，那个端口只在编辑里填写。

左侧还有两页：

- **Tailscale** 列出内网里的机器、内网地址，以及 Online 或 Offline。地址可以复制。这台电脑标成「本机」。
- **设置** 里是配置文件的路径，可以换成别的 JSON 文件。

## 怎么跑起来

需要本机已安装 Node、Rust 和 OpenSSH。

```bash
npm install
npm run tauri dev
```

打开后先新建一条隧道，填主机、SSH 端口、用户、密码和本地转发，保存后再点启动。

## 它会记住什么

配置和密码写在本机文件里，默认是：

`~/Library/Application Support/com.tailtunnel.app/tunnels.json`

关掉窗口或退出程序时，这次启动的 SSH 子进程会一起停掉。程序不会把这些进程留成后台服务，也不会在下次打开时按旧进程号去结束别的程序。

这台 Mac 上如果没有 Tailscale，Tailscale 页只说明没有安装，不提供安装。

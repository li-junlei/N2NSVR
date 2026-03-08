# n2n Supernode Launcher

一个基于 Rust + Tauri 的 Windows 图形化 `supernode` 启动器。

## 功能

- 内置 `supernode.exe`，随安装程序一起分发
- 支持监听端口、管理端口、额外参数配置
- 支持“快速重连”模式，映射到 `supernode -M`
- 支持配置点击关闭按钮时是直接退出还是最小化到系统托盘
- 实时显示 stdout / stderr 日志
- 保存配置并一键启动 / 停止
- 修复中文显示问题，使用适合中文 Windows 的字体栈

## 二进制来源

内置 `supernode.exe` 来自 `lucktu/n2n` 的 Windows 预编译包。

- 仓库: <https://github.com/lucktu/n2n>
- 采用文件: `Windows/n2n_v3_windows_x64_v3.1.1_r1255_static_by_heiye.zip`

这是官方 `ntop/n2n` README 中提到的 Windows 预编译来源之一，但不是 `ntop/n2n` 官方直接发布的二进制。

## 开发运行

```powershell
cargo tauri dev
```

## 打包

安装版：

```powershell
cargo tauri build
```

## 说明

- supernode 负责节点发现和中继，不负责分配虚拟 IP 或虚拟网段
- community、虚拟 IP、网段等配置属于 `edge` 侧，不属于本应用范围
- `-M` 会关闭 supernode 的 MAC/IP 冒用保护，适合可信网络下解决客户端异常断线后的快速重连问题
- 当前分发方式为安装包，`supernode.exe` 通过 Tauri 资源机制一并安装

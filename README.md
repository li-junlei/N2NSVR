# N2NSVR

一个基于 Rust + Tauri 的 Windows 图形化工具，用于管理 **n2n Supernode 服务端**与 **SakuraFRP 内网穿透**。

## 功能

### N2N Supernode
- 内置 `supernode.exe`，随安装程序一起分发
- 支持监听端口、管理端口、额外参数配置
- 支持"快速重连"模式（`supernode -M`）
- 实时显示 stdout / stderr 日志，一键启动 / 停止
- 支持保存配置

### SakuraFRP 内网穿透
- 内置 `frpc_windows_amd64.exe`（Windows 64位），无需另行下载
- 使用访问密钥（Token）通过 API 拉取账号下的全部隧道列表
- 勾选方式选择要启动的隧道，无需手填 ID
- 支持与 N2N 服务端**联动启动**：启动 supernode 时自动同步启动选定的 frpc 隧道
- 支持自定义 frpc 路径（使用自行下载的版本）
- 实时显示 frpc 日志，独立启动 / 停止控制

### 软件行为
- 支持最小化到系统托盘（可配置关闭按钮行为）
- **退出软件时自动终止所有子进程**（supernode + frpc）

## 二进制来源

| 文件 | 来源 |
|------|------|
| `supernode.exe` | `lucktu/n2n` Windows 预编译包（`n2n_v3_windows_x64_v3.1.1_r1255_static_by_heiye.zip`）<br>仓库: <https://github.com/lucktu/n2n> |
| `frpc_windows_amd64.exe` | SakuraFRP 官方分发，仅支持当前内置版本；如需更新请到 <https://www.natfrp.com/> 下载后在设置中指定自定义路径 |

> `supernode.exe` 是官方 `ntop/n2n` README 中提到的 Windows 预编译来源之一，但不是 `ntop/n2n` 官方直接发布的二进制。

## 开发运行

```powershell
cargo tauri dev
```

## 打包

```powershell
cargo tauri build
```

## 注意事项

- supernode 负责节点发现和中继，不负责分配虚拟 IP 或虚拟网段
- community、虚拟 IP、网段等配置属于 `edge` 侧，不属于本应用范围
- `-M` 会关闭 supernode 的 MAC/IP 冒用保护，适合可信网络下解决客户端异常断线后的快速重连问题
- SakuraFRP 访问密钥（Token）与账号登录密码**不同**，请在 [用户面板](https://www.natfrp.com/user/) 中查看

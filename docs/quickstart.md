# swarmx 快速上手

## swarmx 是什么

swarmx 是一个浏览器仪表盘，在 PTY 下拉起你本机已有的真实 `claude`、`codex`、`opencode`、`reasonix` CLI，把它们组成一个协作 swarm。你只需用自然语言描述任务，orchestrator 会负责派出 worker；agent 之间通过共享收件箱和黑板互相通信。

swarmx 不是另一个 LLM 封装：你的 OAuth、套餐限制、速率限制全部原样生效——因为跑的就是你终端里的那个 binary。swarmx 从不读取或存储你的 token。

---

## 前置条件

| 依赖 | 版本要求 |
|------|---------|
| Rust（含 Cargo） | **1.83+** |
| Node.js（含 npm） | **22+** |
| claude CLI | 已登录（复用 `~/.claude/` 凭证） |
| codex CLI（可选） | **0.132+**（低版本 auto-wake 循环不可用） |
| opencode / reasonix（可选） | 已安装即可 |

> swarmx 通过把 `HOME` / `PATH` 透传给子进程来复用你已有的 CLI 凭证，自己从不持久化任何 token。

---

## 方式一：开发模式（从仓库源码跑）

适合想看代码、调试，或者还没有安装包的情况。

### 第一步：克隆并构建

```bash
git clone https://github.com/curdx/swarmx.git
cd swarmx

# 先整体构建——缺少 shim 二进制，后端服务会启动失败
cargo build --workspace
```

### 第二步：安装前端依赖

```bash
# 在 web/ 目录下安装（/tmp 缓存可绕过沙箱 EACCES 错误）
cd web
npm_config_cache=/tmp/.npm-swarmx npm install
cd ..
```

### 第三步：分两个终端启动

**终端 1 — 后端**（必须在仓库根目录执行）：

```bash
cargo run -p swarmx-server
# 监听 127.0.0.1:7777
```

**终端 2 — 前端**：

```bash
cd web
npm run dev
# 监听 http://localhost:5173
```

### 第四步：打开仪表盘

浏览器访问 **http://localhost:5173**，即可使用。

---

## 方式二：桌面安装包（Tauri）

适合普通用户——下载、安装、打开，全程零命令行。

如需自己打包：

```bash
cd web

# 1. 编译 release 后端二进制并复制为 Tauri sidecar
npm run sidecar:release

# 2. 打出真实安装包（.app / .dmg / 等）
npm run tauri:build
```

生成的安装包已将 `swarmx-server`、`swarmx-shim`、`swarmx-mcp` 三个二进制以及所有运行时资源打包进去，无需额外配置。

---

## 第一次使用

1. 打开仪表盘（http://localhost:5173 或安装包）
2. 点击 **新建空间**，选择你的项目目录
3. 在 orchestrator 对话框里用自然语言描述任务，例如：
   > "帮我给这个项目写单元测试，覆盖 src/lib.rs 里的所有公开函数"
4. orchestrator 会自动按需派出 worker（frontend、backend、test-runner 等角色），你可以在仪表盘看到每个 agent 的实时终端输出

---

## 常见坑

### 后端启动失败：找不到 shim 二进制

**现象**：`cargo run -p swarmx-server` 后新建空间报错或服务无法正常工作。

**原因**：`swarmx-shim` 还没有编译出来。

**解决**：先跑 `cargo build --workspace`，再启动后端。

---

### npm install 报 EACCES（权限错误）

**现象**：在沙箱或受限环境下 `npm install` 提示无法写缓存目录。

**解决**：

```bash
npm_config_cache=/tmp/.npm-swarmx npm install
```

---

### 后端必须从仓库根目录启动

**现象**：从其他目录 `cargo run -p swarmx-server` 可能找不到资源。

**原因**：开发模式下，服务会从 CWD 就近查找 `spells/`、`roles/`、`cli-plugins/` 目录。

**解决**：始终在仓库根目录执行 `cargo run -p swarmx-server`。

> 打包版（Tauri sidecar）不受此限制——运行时资源已通过 `include_str!` 编译进二进制。

---

## TODO / 待确认

- [ ] **端口是否可配置**：文档写死了 7777（后端）和 5173（前端），但不确定是否有环境变量可覆盖，需核对 `swarmx-server` 的启动参数或 README。
- [ ] **安装包下载地址**：README 提到"download → install → open"，但未给出 Release 下载链接，需补充或确认 GitHub Releases 页面是否公开。
- [ ] **codex 0.132+ 要求的出处**：README 里提到，但未说明低版本具体哪个功能失效，可以补充更多说明帮助用户判断。
- [ ] **opencode / reasonix 的额外登录步骤**：是否需要单独 `opencode auth` 或类似命令，还是直接复用系统凭证？文档未覆盖。
- [ ] **Windows / Linux 安装包格式**：当前文档只提到 `.app`/`.dmg`（macOS），Windows/Linux 的格式（`.exe`、`.AppImage` 等）需补充。

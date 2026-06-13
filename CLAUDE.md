# flockmux — 项目工作约定

## 头号原则:永远站在「装完包的真实用户」角度验证,而不是开发机

这个项目最容易犯、也最伤用户的一类错误是:**在仓库根目录跑得好好的,打成安装包发出去就是坏的。**
原因是开发时服务从仓库根目录启动,`spells/`、`roles/`、`cli-plugins/` 这些运行时资源就在手边;
而真实用户装的是 Tauri 打的包,服务作为 sidecar 运行时:

- 当前工作目录(CWD)是 `/`,不是仓库根目录;
- 没有任何 `FLOCKMUX_*` 环境变量;
- `env!("CARGO_MANIFEST_DIR")` 指向的是**构建机器**上的路径,在用户电脑上根本不存在。

所以任何「用相对路径 / `CARGO_MANIFEST_DIR` 去找的文件或目录」在用户机上都找不到。
**典型事故:** 安装版点「新建空间」报 “后端未加载 `init` spell” —— 因为 `spells/`/`roles/`/`cli-plugins/`
根本没被打进 `.app`(`tauri.conf.json` 只有 `externalBin`,没有 `resources`;`build-sidecar.sh` 只拷二进制)。
结果新用户装完什么都干不了,还得手动敲命令 —— 这是绝不允许的。

### 因此,任何改动落地前必须自问:

1. **这段逻辑依赖的文件/目录,在用户安装版里真的存在吗?** 不确定就去翻打包配置,别假设。
2. **新用户的完整路径是:下载 → 安装 → 打开 → 立刻能用,全程零命令。** 任何要求用户手动跑命令的方案都是 bug,不是 feature。
3. 运行时需要读的资源,要么编译期 `include_*!`/`sqlx::migrate!` 嵌进二进制,要么作为 Tauri `bundle.resources` 打进包并在启动 sidecar 时用环境变量把绝对路径传进去。两者必居其一。

## 运行时资源 / 打包清单(发版前逐项核对)

服务端运行时会从磁盘读取的资源目录(必须随包发出,不能依赖仓库布局):

- `spells/` —— spell 注册表(含关键的 `init`),启动时读一次,无热重载
- `roles/` —— 角色注册表(含 orchestrator)
- `cli-plugins/` —— CLI 插件(含 `claude`)
- 其余依赖以代码实际情况为准(发现新增的运行时资源目录,同步补进这份清单和打包配置)

解析顺序见 `crates/flockmux-server/src/{spells,roles,plugins}.rs` 的 `default_*_dir()`:
`FLOCKMUX_*_DIR` 环境变量 > `CARGO_MANIFEST_DIR` 相对路径 > 裸相对路径。
**用户机上只有第一条(环境变量)可靠** —— 所以 Tauri 启动 sidecar 时必须把这些环境变量指到打包进去的资源目录。

## 发版 = 必须验证安装版本身

不要只验证 `cargo run` / `tauri dev`。每次发版前,至少在本机:

1. `tauri build` 出真实安装包;
2. 确认 `.app/Contents/Resources/`(及 Windows/Linux 对应位置)里这些资源目录都在;
3. 启动安装版,确认「新建空间」能跑通(或至少 `/api/spells` 能列出 `init`),全程不碰命令行。

发版脚本与流程见 `scripts/bump-version.mjs`、`.github/workflows/release.yml`、`web/src-tauri/scripts/build-sidecar.sh`。

## 开发环境

- 先 `cargo build --workspace`(缺 shim 二进制服务会启动失败)。
- 前端依赖:`web/` 下 `npm_config_cache=/tmp/.npm-flockmux npm install`(绕沙箱 EACCES)。
- 端口:后端 7777 / 前端 5173。
- debug 构建下 Tauri **不**自动拉起服务,需自己 `cargo run -p flockmux-server`(从仓库根目录,才能就近找到资源目录)。

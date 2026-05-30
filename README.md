# BeeAPI Switch

BeeAPI Switch 是一款面向 AI 编程命令行工具的桌面配置器。它把密钥、模型、接入地址和本地代理能力集中到一个界面里，帮助你把常用 AI Coding CLI 快速接入 BeeAPI。

项目基于 Tauri + React 构建，默认接入地址为 `https://beeapi.ai/`。

## 它解决什么问题

不同 CLI 工具的配置文件位置、接口格式和鉴权方式各不相同。BeeAPI Switch 负责把这些差异收拢起来：

- 自动识别工具安装状态。
- 自动写入或恢复各工具配置。
- 统一管理 API Key、模型和接入模式。
- 通过本地代理提供密钥池、失败切换和请求统计。
- 提供演练场，方便直接测试对话和图片生成。

## 功能概览

### 工具配置

- 一键配置 Claude Code、Codex、Gemini CLI、OpenCode、OpenClaw、Hermes。
- 支持直连上游接口，也支持通过本地代理接入。
- 写入配置前自动备份，支持在界面中恢复、回滚和清理。
- 支持拉取模型列表并写入目标模型。

### 密钥池与本地代理

- 支持多密钥管理、启用/禁用、权重配置。
- 本地代理自动轮询可用密钥。
- 请求失败时支持故障切换和冷却。
- 记录请求日志、延迟、状态码和 Token 用量。

### 检查与统计

- 支持账号余额/额度查询。
- 支持健康检查：模型列表、普通请求、流式请求。
- 支持本地会话统计和 Markdown 导出。
- 支持 Codex 历史会话修复与插件入口解锁辅助。

### 演练场

- 支持对话测试。
- 支持文本生图。
- 支持图生图：上传参考图、预览、移除后发送。
- 支持保存生成图片和查看大图。

### 应用体验

- 支持中文和英文界面。
- 支持深色/浅色主题。
- 支持 Windows、macOS、Linux 自动构建发布。
- 支持 Tauri 在线更新。

## 支持的工具

| 工具 | 配置文件 | 接口类型 |
| --- | --- | --- |
| Claude Code | `~/.claude/settings.json` | Anthropic 兼容 |
| Codex | `~/.codex/config.toml`、`~/.codex/auth.json` | OpenAI / Responses API |
| Gemini CLI | `~/.gemini/settings.json`、`~/.gemini/.env` | Gemini / OpenAI 兼容 |
| OpenCode | `~/.config/opencode/opencode.json` | OpenAI 兼容 |
| OpenClaw | `~/.openclaw/config.json` | Anthropic 兼容 |
| Hermes | `~/.hermes/config.yaml` | OpenAI 兼容 |

## 配置和数据安全

写入任何 CLI 配置前，BeeAPI Switch 会自动备份原文件到：

```text
~/.beeapi-switch/backups/<tool>/<timestamp>/
```

密钥池和本地设置默认保存在：

```text
~/.beeapi-switch/settings.json
```

请不要把这个文件、`.tauri/` 目录或任何签名私钥上传到公开环境。

## 下载与更新

Release 页面会提供各平台安装包：

- Windows x64：NSIS 安装包
- macOS x64 / Apple Silicon：DMG
- Linux x64：AppImage

应用内在线更新使用 GitHub Release 中的 `latest.json`：

```text
https://github.com/PeterGriffin888/beeapi-switch/releases/latest/download/latest.json
```

## 开发环境

推荐环境：

1. Node.js 18+
2. Rust stable toolchain
3. 对应平台的 Tauri 构建依赖

Windows 额外需要：

- Microsoft Edge WebView2 Runtime
- Visual Studio Build Tools，包含 “Desktop development with C++” 工作负载

Linux 额外需要 WebKitGTK、AppIndicator、GTK 等依赖。GitHub Actions 中已有示例安装命令。

安装依赖：

```bash
npm install
```

启动开发模式：

```bash
npm run tauri dev
```

构建前端：

```bash
npm run build
```

## 本地打包

Windows NSIS：

```bash
npm.cmd run tauri -- build --bundles nsis
```

带在线更新签名打包：

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY = Get-Content -Raw .tauri\updater.key
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = Get-Content -Raw .tauri\updater.key.password
npm.cmd run tauri -- build --bundles nsis
node scripts/make_latest_json.cjs
```

常见产物位置：

```text
src-tauri/target/release/beeapi-switch.exe
src-tauri/target/release/bundle/nsis/BeeAPI Switch_<version>_x64-setup.exe
src-tauri/target/release/bundle/nsis/BeeAPI Switch_<version>_x64-setup.exe.sig
release/update/latest.json
```

## 自动发布

项目使用 GitHub Actions 自动构建和发布。推送 `v*` 标签会触发全平台构建：

```bash
git tag v0.1.0
git push origin v0.1.0
```

发布前需要在仓库的 `Settings -> Secrets and variables -> Actions` 中配置：

| Secret | 说明 |
| --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | Tauri updater 私钥内容 |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | 私钥密码 |

Actions 会完成：

1. 分别构建 Windows、macOS、Linux 产物。
2. 生成各平台 updater 签名。
3. 汇总生成 `latest.json`。
4. 创建 GitHub Release 并上传所有产物。

## 项目结构

```text
beeapi-switch/
├─ src/                     React 前端
│  ├─ App.tsx               主界面
│  ├─ KeysView.tsx          密钥池管理
│  ├─ PlaygroundView.tsx    演练场
│  ├─ UsageView.tsx         使用统计
│  ├─ SessionsView.tsx      会话管理
│  ├─ AboutView.tsx         关于与在线更新
│  └─ i18n.ts               中英文文案
├─ src-tauri/
│  ├─ src/
│  │  ├─ lib.rs             Tauri 命令与应用入口
│  │  ├─ config.rs          配置写入、备份与状态检测
│  │  ├─ tools.rs           各 CLI 配置适配器
│  │  ├─ proxy.rs           本地代理与密钥池路由
│  │  ├─ pool_store.rs      本地设置持久化
│  │  └─ usage.rs           会话用量统计
│  ├─ capabilities/         Tauri 权限配置
│  ├─ icons/                应用图标
│  ├─ tauri.conf.json       Tauri 配置
│  └─ Cargo.toml            Rust 依赖
├─ scripts/                 发布和辅助脚本
├─ package.json             前端依赖与命令
└─ vite.config.ts           Vite 配置
```

## 更新记录

### v0.1.0

BeeAPI Switch 发布 ！！

## 参考项目

- [farion1231/cc-switch](https://github.com/farion1231/cc-switch)
- [jlcodes99/cockpit-tools](https://github.com/jlcodes99/cockpit-tools)
- [BigPizzaV3/CodexPlusPlus](https://github.com/BigPizzaV3/CodexPlusPlus)

## 注意事项

- Codex 插件解锁会关闭当前 Codex 进程并重新启动注入，请先保存未完成内容。
- 健康检查会真实请求上游接口，可能产生极少量 Token 消耗。
- macOS 未签名/未公证产物可能触发系统安全提示。

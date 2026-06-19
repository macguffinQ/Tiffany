# tiffany-loop

[![CI](https://github.com/macguffinQ/Tiffany/actions/workflows/ci.yml/badge.svg)](https://github.com/macguffinQ/Tiffany/actions/workflows/ci.yml)
[![Release](https://github.com/macguffinQ/Tiffany/actions/workflows/release.yml/badge.svg)](https://github.com/macguffinQ/Tiffany/actions/workflows/release.yml)

[English](README.md) | [简体中文](README.zh-CN.md)

> 一个用 Rust 编写的轻量级多智能体编排平台，把 **Claude Code**、**Codex CLI** 以及其他具备工具能力的 LLM CLI 统一到同一套适配器下，并提供类似 Claude Code 的终端对话界面。

**状态：** Preview / Beta。`1.0` 之前 CLI、配置格式、TUI 行为和发布包结构都可能继续调整。

正式产品名是 **tiffany-loop**。当前安装命令仍保留 `tiffany` 和 `orchestrator`，用于兼容已有脚本和用户工作流。

这是一个独立社区项目，不是 OpenAI、Anthropic、MiniMax 或任何模型/运行时提供商的官方产品。第三方名称只用于说明兼容工具、provider 和上游开源组件。

```bash
$ ./scripts/tiffany-dev
tiffany-loop orchestrator
tiffany-loop orchestration mode

> implement fib
... planner / critic / Claude worker / reviewer ...
```

## 这是什么？

`tiffany-loop` 用来把多个 AI 编程智能体组织成一条可观察、可审查、可追踪的工作流。你输入一个问题，它可以通过配置好的角色完成规划、批评、执行和复核。

核心流程是：

```text
用户任务
  -> Planner 规划任务
  -> Critic 批评/反驳计划
  -> Router 选择执行角色
  -> Worker 并行执行
  -> Reviewer 复核结果
  -> 输出最终结果并记录会话
```

它适合这些场景：

- 想让多个 AI 编程工具协作，而不是只和一个模型聊天。
- 想把复杂任务拆给不同角色，例如“指挥者、批评者、执行者 Claude”。
- 想看到执行过程，而不是只拿到一个黑盒最终答案。
- 想保留历史对话、过程日志、会话上下文和交接包。
- 想在终端里完成工作，保留原生滚动、选中、复制、粘贴和中文输入体验。

## 到底该运行哪个命令？

正常交互使用：安装后先运行一次 `orchestrator setup`，再用 `tiffany orchestrator` 进入主界面；源码开发时用 `./scripts/tiffany-dev setup` 和 `./scripts/tiffany-dev`。

| 名称 | 它是什么 | 用途 |
|---|---|---|
| `tiffany-loop` | 产品名 / 包名 | Release、Homebrew、文档、GitHub 项目标识 |
| `tiffany` | 主 TUI 二进制 | 交互对话、provider 设置、角色设置、多轮编排 |
| `orchestrator` | runtime / 控制二进制 | 配置、事件流、ACP server、非交互运行、脚本化 |
| `./scripts/tiffany-dev` | 源码开发辅助脚本 | 不安装二进制时从当前 checkout 启动 |

这个拆分是有意的：`tiffany` 负责终端交互体验，`orchestrator` 负责
provider 配置、角色路由、事件流和 runtime 执行。`orchestrator tui` 在
安装了 `tiffany` 时会自动转到 `tiffany orchestrator`。

## UI 方向：直接基于 tiffany-loop UI

当前主 UI 路线已经切到完整 tiffany-loop UI：[`tiffany-ui/`](tiffany-ui/)。后续终端界面改动都应在这个 fork 里做，保留上游 Ratatui/Crossterm 架构、窗口缩放、选中复制、历史 cell、bottom pane、overlay 和退出渲染策略。

旧的 [`src/tui`](src/tui/) 只作为现有 orchestrator runtime 的兼容桥接层保留。除非是迁移前必须修的窄 bug，不再继续手写终端渲染。

开发入口：

- `./scripts/tiffany-dev`：默认进入 tiffany-loop orchestrator 的 tiffany-loop orchestration mode，首次缺失时构建 `./target/debug/orchestrator` 和 `./target/debug/tiffany`，之后复用 debug binary 直接启动，打开后等待输入。
- `./scripts/tiffany-dev --help`：只显示源码 checkout 的入口说明，不构建、不启动 UI。
- `./scripts/tiffany-dev setup`：不安装二进制，直接运行本工程首次配置向导。
- `./scripts/tiffany-dev config ...`：直接执行本工程 orchestrator config 命令，不会启动 TUI，也不要求 UI 登录；适合进 TUI 前先脚本化配置 provider。
- `./scripts/tiffany-dev orchestrator "..."`：带初始问题立即运行 orchestrator 流程；native mode 默认执行者是 Claude Code（`worker-cc`），进入后输入框继续提交也会走 orchestrator。
- `./scripts/tiffany-dev orchestrator --orchestrator-config /path/to/config.yaml`：指定 orchestrator 配置文件，同时继续让 tiffany-loop UI 配置隔离在 `TIFFANY_HOME`。
- `./scripts/tiffany-dev orchestrator --legacy ...`：兼容旧 orchestrator CLI 桥接。
- `./scripts/tiffany-build [cargo-build-args...]`：同时构建父工程 orchestrator 和 tiffany-loop UI，默认共用 `./target`；需要更快的可分发构建时用 `--fast-release`，最终二进制默认 strip，可用 `TIFFANY_NO_STRIP=1` 保留符号。需要构建后只保留最终 dist 二进制时，加 `--prune-dist-cache`。
- `./scripts/tiffany-check --smoke`：debug 构建、检查 fork 格式，并验证 legacy bridge 和事件流入口。
- `./scripts/tiffany-check --dist`：用可分发的 `tiffany-dist` profile 跑同样检查，发布前使用。
- `./scripts/tiffany-clean-targets --sizes|--top|--top-deep|--incremental|--dist-cache|--dist|--debug`：查看构建缓存大小、定位大文件，保留最终 dist 二进制但删除 dist 中间缓存，或清理 incremental/release/debug 构建产物，避免 `target/` 膨胀。

直接进入 `tiffany-ui/codex-rs` 执行 Cargo 命令时，也会通过 fork 的 Cargo 配置重定向到根目录 `./target`。旧 checkout 如果已经有 `tiffany-ui/codex-rs/target`，那是重复缓存，可以用 `./scripts/tiffany-clean-targets` 删除。

源码开发时，`./scripts/tiffany-dev` 默认复用已构建的 debug binary，避免每次启动都走 `cargo run`。只有明确需要 Cargo wrapper 时再设置 `TIFFANY_DEV_CARGO_RUN=1`。

tiffany-loop 的 fork 状态和上游 UI 分离。默认使用 `TIFFANY_HOME=~/.tiffany`，tiffany-loop 内部配置读取会被映射到 `~/.tiffany/config.toml`，不会读写上游默认配置目录。SQLite 状态库默认也通过 `TIFFANY_SQLITE_HOME` 指到同一目录。需要多套配置时可以用 `TIFFANY_HOME=/path/to/tiffany-home` 覆盖。

运行源码辅助脚本或直接运行 fork binary 时，可以设置 `TIFFANY_ORCHESTRATOR_BIN=/path/to/orchestrator`，也可以传 `tiffany orchestrator --bin /path/to/orchestrator`。
安装后的发布包会同时包含 `orchestrator` 和 `tiffany`。`orchestrator tui` 会优先转到 `tiffany orchestrator`，只有找不到 `tiffany` 时才回退到旧终端对话；需要强制旧入口时设置 `ORCHESTRATOR_LEGACY_TUI=1`。

## 主要特性

- **角色编排**：Planner、Critic、Worker、Reviewer、Router、A/B Judge。
- **多运行时**：Claude Code、Codex CLI、直接 API。
- **多模型/多提供商**：Anthropic、OpenAI、Google Gemini、Ollama、本地或 OpenAI 兼容端点。
- **终端 TUI**：主线切到完整 tiffany-loop UI；旧 `orchestrator tui` 仅保留兼容。
- **tiffany-loop 原生事件流**：`tiffany orchestrator "..."` 把 planner、critic、worker、reviewer 和最终结果写入 tiffany-loop history cell。
- **原生多轮编排**：在 orchestrator mode 下，tiffany-loop 输入框提交会被路由到 orchestrator adapter，不再走普通 tiffany-loop 模型回合。
- **过程透明**：灰色滚动展示运行过程，`/o` 可折叠或展开后续过程详情。
- **最终结果清晰**：最终输出为纯文本结果块，方便选中复制；`/result` 可重新输出完整结果。
- **队列与多轮**：运行中继续输入会进入 tiffany-loop 底部 pending queue，普通消息在当前轮结束后合并为下一批一起执行。
- **上下文记忆**：支持紧凑/完整/关闭/清空上下文。
- **交接能力**：可生成 Claude/Codex CLI handoff 包，切到对应 CLI 继续工作。
- **ACP**：提供 Agent Client Protocol stdio server，可被支持 ACP 的客户端调用。
- **会话日志**：JSONL + SQLite 索引，便于复盘、搜索、注入上下文。

## 安装

普通用户推荐用 Homebrew：

```bash
brew tap macguffinQ/tap
brew install tiffany-loop
orchestrator setup
tiffany orchestrator
```

Homebrew 包会同时安装两个命令：

- `tiffany`：主终端 UI，启动用 `tiffany orchestrator`。
- `orchestrator`：配置、角色、事件流、ACP 和非交互运行。

安装后：

```bash
orchestrator init
orchestrator setup
orchestrator doctor
tiffany orchestrator
```

进入 TUI 后，用 `/provider` 配置 provider，用 `/role` 注册 planner、critic、worker、reviewer 等角色。

每个 `v*` tag 发布后，GitHub Releases 会优先提供 macOS Apple Silicon 预编译二进制，压缩包内也包含两个命令。Linux、Windows 和 Intel Mac 目前可先从源码安装，后续再补更多预编译目标。

贡献者源码运行：

```bash
git clone https://github.com/macguffinQ/Tiffany.git ~/code/orchestrator
cd ~/code/orchestrator
./scripts/tiffany-build
./scripts/tiffany-dev setup
./scripts/tiffany-dev
```

如果不用 Homebrew，又想把命令安装进 `PATH`：

```bash
cargo install --path . --profile tiffany-dist
cargo install --path tiffany-ui/codex-rs/tiffany-cli --profile tiffany-dist
strip "$(command -v orchestrator)" "$(command -v tiffany)" 2>/dev/null || true
```

```bash
# 示例：安装下载好的 macOS Apple Silicon 压缩包
tar -xzf tiffany-loop-v0.1.11-aarch64-apple-darwin.tar.gz
cd tiffany-loop-v0.1.11-aarch64-apple-darwin
chmod +x orchestrator tiffany
./orchestrator setup
./orchestrator doctor
./tiffany orchestrator
```

`tiffany-loop` 安装包会提供 `orchestrator` 和 `tiffany` 两个二进制；当前 GitHub 仓库名仍是 `Tiffany`。
tag release 目前优先提供 Homebrew 使用的 macOS Apple Silicon 压缩包；Intel Mac、Linux 和 Windows 可以先从源码安装。
公开 Homebrew 安装要求 `macguffinQ/Tiffany` 仓库和 release assets 对外公开。

## 快速开始

```bash
# 1. 初始化配置
orchestrator init
# 写入 ~/.orchestrator/config.yaml

# 2. 配置 provider、model、role
orchestrator setup
# 也可以进入 TUI 后用 /provider 和 /role

# 3. 检查 provider -> model -> role -> runtime 是否连通
orchestrator doctor

# 4. 进入主 TUI
cd ~/your-project
tiffany orchestrator

# 5. 或者运行单次非交互任务
orchestrator run "implement fibonacci in src/fib.rs"

# 6. 从源码打开 tiffany-loop UI
./scripts/tiffany-dev

# 7. 查看历史会话
orchestrator sessions list
orchestrator sessions show <id|prefix|last>          # 默认人类可读
orchestrator sessions show <id|prefix|last> --raw    # 原始 JSONL
orchestrator sessions show <id|prefix|last> --tree   # 父子 run 树
orchestrator sessions show <id|prefix|last> --flow   # 总控/worker 可读瀑布流
orchestrator sessions grep "rate limit"
```

## 配置

tiffany-loop 有两套配置根目录：

- Orchestrator runtime：`~/.orchestrator/config.yaml`
- tiffany-loop UI：默认 `~/.tiffany/config.toml`，或 `$TIFFANY_HOME/config.toml`；SQLite 状态库使用 `$TIFFANY_SQLITE_HOME` 或同一目录

默认配置文件位于：

```text
~/.orchestrator/config.yaml
```

可从 `config.example.yaml` 开始：

```yaml
providers:
  anthropic: { type: anthropic, api_key: ${ANTHROPIC_API_KEY} }
  openai:    { type: openai,    api_key: ${OPENAI_API_KEY} }
  minimax:   { type: openai,    api_key: ${MINIMAX_API_KEY}, base_url: https://api.minimaxi.com/v1 }
  google:    { type: google,    api_key: ${GOOGLE_API_KEY} }
  ollama:    { type: ollama,    base_url: http://localhost:11434 }

models:
  - { id: opus,        provider: anthropic, name: claude-opus-4-6 }
  - { id: sonnet,      provider: anthropic, name: claude-sonnet-4-6 }
  - { id: gpt4o,       provider: openai,    name: gpt-4o }
  - { id: gpt4o-mini,  provider: openai,    name: gpt-4o-mini }
  - { id: minimax-m3-claude, provider: minimax, name: MiniMax-M3 }
  - { id: minimax-m3-codex,  provider: minimax, name: MiniMax-M3 }

roles:
  planner:      { model: gpt4o-mini, runtime: codex }
  critic:       { model: opus,       runtime: claude-code }
  worker-cc:    { model: sonnet,     runtime: claude-code, agent_teams: true }
  worker-codex: { model: gpt4o,      runtime: codex }
  reviewer:     { model: gpt4o-mini, runtime: codex }

behavior:
  worktree_base:   ~/.orchestrator/worktrees
  db_path:         ~/.orchestrator/state.db
  session_log_dir: ~/.orchestrator/sessions
  mux:             zellij
  enable_critic:   true
  enable_reviewer: true
  max_replan:      2
  cc_bypass_permissions: true          # Claude Code 角色/worker 非交互执行
```

`cc_bypass_permissions` 会给 tiffany-loop 启动的 Claude Code 子进程传 `--permission-mode bypassPermissions`。只有想让 Claude Code 手动停下来选择权限时才设为 `false`；tiffany-loop TUI 里的 `/permissions` 只管 tiffany-loop UI 自己，不管这些 Claude 子进程。

角色解析优先级：

| 优先级 | 来源 | 示例 |
|---|---|---|
| 1 | CLI 参数 | `--worker worker-cc` |
| 2 | 任务标签 | `--tag refactor` -> `worker-cc` |
| 3 | 默认配置 | `roles.worker-cc.model = sonnet` |

缺失环境变量会被展开为空字符串，因此没有 API Key 时也可以先运行 `orchestrator status` 检查状态。

## 常用命令

| 命令 | 作用 |
|---|---|
| `./scripts/tiffany-dev` | 运行 tiffany-loop UI |
| `./scripts/tiffany-dev setup` | 从源码运行首次配置向导 |
| `./scripts/tiffany-dev orchestrator` | 从 tiffany-loop fork 桥接到现有 orchestrator runtime |
| `./scripts/tiffany-build [args]` | 在共享 `./target` 中同时构建两个二进制；可透传 `--release --locked`，也可用默认 strip 的 `--fast-release --locked`；加 `--prune-dist-cache` 可只保留最终 dist 二进制 |
| `./scripts/tiffany-check --smoke` | 执行快速本地 fork/bridge 验证 |
| `./scripts/tiffany-check --dist` | 执行发布 profile 的 fork/bridge 验证 |
| `./scripts/tiffany-clean-targets --sizes|--top|--top-deep|--incremental|--dist-cache|--dist|--debug` | 查看或精简共享/旧 Cargo 构建缓存；默认只删除旧 fork-local target |
| `tiffany orchestrator` | 安装后打开主 tiffany-loop UI |
| `orchestrator init` | 生成 `~/.orchestrator/config.yaml` |
| `orchestrator setup` | 引导式配置 provider、model、role |
| `orchestrator run "..."` | 执行一个任务 |
| `orchestrator config provider setup <provider>` | 按预设配置 provider |
| `orchestrator config provider list|presets` | 查看 provider 配置或内置预设 |
| `orchestrator roles list` | 查看已注册角色 |
| `orchestrator roles register <role> --model <id> --runtime <runtime>` | 注册或更新角色绑定 |
| `orchestrator tui` | 找到 `tiffany` 时打开主 tiffany-loop UI；否则回退到旧终端对话 |
| `orchestrator tui --ratatui` | 旧兼容参数；仅保留给老脚本 |
| `orchestrator tui --new-tab` | 在 zellij 中打开新 tab |
| `orchestrator tui --detach` | 后台启动终端对话 |
| `orchestrator acp` | 启动 Agent Client Protocol stdio server |
| `orchestrator sessions list` | 列出历史会话，显示角色、父子关系提示和可复制打开命令 |
| `orchestrator sessions show <id|prefix|last>` | 查看人类可读的总控或 worker 会话日志，`--raw` 保留原始 JSONL，`--tree` 显示父子关系，`--flow` 显示可读瀑布流 |
| `orchestrator sessions grep <pattern>` | 搜索会话日志并显示可读摘要 |
| `orchestrator sessions import-cc` | 导入 Claude Code 历史会话 |
| `orchestrator config` | 查看和修改 orchestrator 配置 |
| `orchestrator status` | 查看两个安装命令、配置根目录、桥接命令、mux 和日志路径 |
| `orchestrator doctor` | 诊断 tiffany UI 查找、配置、runtime、API key、角色绑定和本地工具 |

## 终端界面

目标终端界面是 [`tiffany-ui/`](tiffany-ui/) 里的完整 tiffany-loop UI。从源码运行：

```bash
./scripts/tiffany-dev
```

安装后运行：

```bash
tiffany orchestrator
orchestrator tui
```

迁移期间显式传 orchestrator 参数：

```bash
./scripts/tiffany-dev orchestrator
```

`orchestrator tui` 会优先查找同目录或 `PATH` 里的 `tiffany` 并启动 `tiffany orchestrator`。找不到时才使用旧兼容入口；需要强制旧入口时使用：

```bash
ORCHESTRATOR_LEGACY_TUI=1 orchestrator tui
```

显式转发 orchestrator 参数：

```bash
./scripts/tiffany-dev orchestrator status
./scripts/tiffany-dev orchestrator -- --help
```

- 系统滚动条和 scrollback 可用。
- 鼠标选中、复制、粘贴由终端/系统处理。
- 中文输入法保持正常。
- 输入 `/` 会打开命令下拉菜单。
- `Up`/`Down` 可切换历史输入或命令菜单选项。
- `Ctrl+C` 在运行中表示取消任务，空闲时表示退出。

后续替换目标是 tiffany-loop UI adapter，而不是继续扩写旧 TUI。

常用 `/` 命令：

- `/help`：查看命令。
- `/doctor`：诊断配置、runtime、API key、角色绑定和本地工具。
- `/provider [setup|edit <provider>]|list|delete <provider>|env|key|endpoint`：打开或修改 provider 设置表单，查看/删除/配置 provider。
- `/role [<role>|register <role> --model <id> --runtime <runtime>]`：打开角色注册表单，或直接注册一个角色。
- `/roles show|route|use|save`：查看、选择、保存角色路由。
- tiffany-loop UI 原生模式支持 `/provider`：`/provider` 打开借鉴 OpenClaw 的 provider 设置面板，provider、type、env、key、endpoint 分开填写；面板会显示 preset 摘要、auth 状态（环境变量 set/unset、字面量 key 警告、Ollama 无需 key）以及即将执行的 `config provider setup ...` 写入预览；带 `▾` 的字段按 `Space` 或 `F4` 打开可滚动下拉，可用上下键或数字选择，`Enter` 应用；选择 provider 会自动填默认 type/env/endpoint；`/provider edit minimax` 会从现有配置预填；`/provider list` 查看配置，`/provider delete minimax` 删除 provider，`/provider env openai OPENAI_API_KEY` 写入环境变量引用，`/provider endpoint openai https://api.openai.com/v1` 写入 endpoint。
- provider/type、role/provider/model/runtime 这类选择字段已经锁定为下拉选项，不能直接乱输入；key/env/endpoint 保留自由输入。`/role` 里用户只选 `API Model`，orchestrator 内部 model id 自动生成；内置模型会跟随当前 provider 过滤，`custom`/`none` 下允许手动输入模型。
- tiffany-loop UI 原生模式支持 `/role`：`/role` 打开独立角色注册表单，role、provider、model、name、runtime、teams 分开填写；`/role worker-codex` 会预填该角色；`/role register worker-cc --model sonnet --runtime claude-code --agent-teams` 可直接写入 orchestrator 配置。
- Claude Code worker 可以注册多个。`worker-cc` 只是默认示例；`worker-cc-minimax`、`worker-cc-sonnet`、`executor-ui` 这类角色只要 `runtime` 是 `claude-code`，都可以通过 `/roles use <role>` 精确选择。
- tiffany-loop UI 原生模式支持 `/roles`：`/roles` 列出角色，`/roles show critic` 查看单个角色。
- `/workflow`：查看 planner -> critic -> worker -> reviewer 流程。
- `/agent claude|codex|auto`：选择后续 worker 路由。
- `/context compact|full|off|clear`：控制上下文记忆。
- `/process summary|full|200`：查看运行过程捕获。
- `/trace full`：显示更多过程追踪。
- `/queue pause|resume|run|edit n text`：管理排队消息。
- `/result`：输出完整纯文本最终结果，适合选中复制。
- `/copy result`：复制最终结果到剪贴板。
- `/handoff claude|codex`：生成交接包。
- `/continue claude|codex`：保存交接包并切到对应 CLI。
- `/graph compact|full|mermaid|save`：把对话压缩为流程图/摘要。
- `/acp status|claude|codex`：查看 ACP server 和客户端配置提示。
- `/o`：折叠或展开后续过程详情。

底部状态行会常驻显示当前阶段、耗时、worker 路由、上下文模式、队列数量、`/o` 折叠状态、process filter、review/worker 问题计数，尽量保持一行内可扫读。

优先排障：

- 不确定当前用的是哪个 `tiffany` / `orchestrator` binary 或配置根目录时，先运行 `orchestrator status`。
- worker 提前退出、provider 报 `model not found` / `模型不存在` / `401/403`、或者 API key 看起来没生效时，先运行 `/doctor` 或 `orchestrator doctor`。
- doctor 会在不打印密钥的前提下检查环境变量 key 引用，验证 `role -> model -> provider -> runtime` 是否连通，提示重复/缺失 model，并显示本机安装/构建环境：Homebrew tap/package、Rust/cargo、Xcode/CLT 和 worker CLI 二进制。
- 模型报错时，重点确认角色里的内部 model id 是否指向正确的 provider API model name：用 `/role <role>`，或 `orchestrator roles register <role> --model <id> --provider <provider> --model-name <api-model> --runtime <runtime>` 修正。

`tiffany orchestrator "..."` 进入后，直接在 tiffany-loop 输入框继续问即可触发下一轮 orchestrator 编排。运行中输入的普通消息会停留在底部队列，当前任务结束后合并为下一批一起执行。底部最多预览 4 条，完整队列可用 `/queue show` 查看。

建议顺序：先用 `/provider` 配置 provider，再用 `/role` 注册角色；`/roles register ...` 仍保留给命令行式输入。tiffany-loop UI 启动时会读取 `~/.orchestrator/config.yaml`。

Provider 设置示例：

```bash
./scripts/tiffany-dev config provider
./scripts/tiffany-dev config provider ui --dry-run
./scripts/tiffany-dev config provider presets
./scripts/tiffany-dev config provider setup minimax
./scripts/tiffany-dev config provider delete minimax
./scripts/tiffany-dev config provider setup custom --env CUSTOM_API_KEY --endpoint https://llm.example.com/v1
./scripts/tiffany-dev config provider list
```

`config provider` 和 `config provider ui` 会打开引导式选择器：可以选择新增/修改、删除、查看 provider；provider、type、auth、env、endpoint 都能用上下键下拉选择。`config provider setup ...` 和 `config provider delete ...` 保留给脚本和 CI 使用。

TUI 内等价命令：

```text
/provider
/provider edit minimax
/provider list
/provider delete minimax
/provider env openai OPENAI_API_KEY
/provider endpoint openai https://api.openai.com/v1
```

角色注册在独立表单：

```text
/role
/role worker-codex
/role register critic --model gpt4o --runtime codex
```

也可以进 TUI 前直接用脚本写 provider：

```bash
./scripts/tiffany-dev config provider setup minimax
```

已有字面量 key 不会在表单里明文展示，`Key: <unchanged>` 表示保留原值。

角色注册示例：

```bash
orchestrator roles register planner --model gpt4o --runtime codex
orchestrator roles register critic --model glm51 --provider openai --model-name glm-5.1 --runtime codex
orchestrator roles register worker-cc --model minimax-m3 --provider openai --model-name minimax-m3 --runtime claude-code --agent-teams
```

## 读取现有项目上下文

orchestrator 会读取并注入这些上下文：

- Claude Code 的 `CLAUDE.md`
- Claude Code 的 `settings.json`
- Claude Code 的 `.claude/agents/*.md`
- 项目的 `.mcp.json`
- Claude Code 既有 session JSONL
- orchestrator 自己的 `AGENTS.md`

注入优先级：

```text
AGENTS.md > CLAUDE.md > orchestrator history > Claude Code prior sessions
```

## 架构

```text
1. Worker runtime       Claude Code CLI / Codex CLI / direct API
2. MCP tool pool        fs / git / github / slack / ...
3. Adapter layer        ModelProvider + WorkerAdapter
4. Shared state         SQLite task queue + git worktree pool
4.5 Session log         JSONL + SQLite index
5. Orchestrator core    Plan -> Critique -> Route -> Execute -> Review
6. Observability        terminal chat + structured JSON logs
7. Entry layer          CLI / terminal chat / ACP / webhook module
```

更多细节见 [docs/architecture.md](docs/architecture.md)。

## 开发

```bash
# 构建
cargo build
cargo build --release
./scripts/tiffany-build
./scripts/tiffany-build --release --locked
./scripts/tiffany-build --fast-release --locked
./scripts/tiffany-build --fast-release --locked --prune-dist-cache

# 直接在 tiffany-ui/codex-rs 中执行 cargo，也会使用根目录 ./target。
# 用 ./scripts/tiffany-clean-targets --top 定位体积来源；
# 用 ./scripts/tiffany-clean-targets --top-deep 查看较慢的文件级细节；
# 用 ./scripts/tiffany-clean-targets --dist-cache 保留最终 dist 二进制并清理可重建的 dist 中间缓存。

# 测试
cargo test

# 格式化和 lint
cargo fmt
cargo clippy

# 从源码运行
./scripts/tiffany-dev
cargo run -- run "hello"
cargo run -- config
```

## 开源协作

- 贡献说明：[CONTRIBUTING.md](CONTRIBUTING.md)
- 安全策略：[SECURITY.md](SECURITY.md)
- 行为准则：[CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)
- 开源发布检查清单：[docs/open-source-checklist.md](docs/open-source-checklist.md)

请不要提交 API Key、本地配置、`~/.orchestrator` 会话日志、数据库文件或生成的 worktree。

## 路线图

已完成：

- 多角色编排
- Claude Code / Codex CLI 适配
- 单窗口终端对话
- 运行过程捕获
- 排队消息
- 上下文记忆
- 纯文本最终结果
- handoff 到 Claude/Codex CLI
- ACP stdio server
- 对话流程图摘要
- patch checkpoint / rollback
- 完整 tiffany-loop UI：`tiffany-ui/`
- tiffany-loop CLI binary
- `tiffany orchestrator "..."` 原生 tiffany-loop TUI 事件 adapter
- 原生 tiffany-loop 输入框多轮 orchestrator 提交
- `tiffany orchestrator --legacy ...` 兼容旧 runtime
- 已 vendor tiffany-loop TUI 源码快照：`third_party/openai-codex/codex-rs/tui`
- `orchestrator tui` 在安装了 `tiffany` 时默认进入 tiffany-loop UI
- Release/Homebrew 同时安装 `orchestrator` 和 `tiffany`

计划中：

- fork adapter 稳定后删除旧的局部复制 TUI 模块
- 更完整的 token 级最终答案流式输出
- 后台任务和 attach
- Session 导出为 Markdown/HTML
- 成本预算告警
- VS Code 扩展

## License

MIT

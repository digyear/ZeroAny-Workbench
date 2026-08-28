<p align="center">
  <img src="assets/brand/zeroany-workbench-wordmark.png" alt="ZeroAny Workbench" width="520" />
</p>

<p align="center">
  <strong>The AI-powered super-app for developers.</strong>
</p>

<p align="center">
  Coding agents · workspace · REST · SQL · NoSQL · SSH · file explorer<br/>
  — every tool, one shell, an AI tuned to each.
</p>

<p align="center">
  <a href="https://github.com/digyear/ZeroAny-Workbench/releases/latest"><img src="https://img.shields.io/github/v/release/digyear/ZeroAny-Workbench?style=flat-square&color=ff5436&label=latest" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-7c5cf8?style=flat-square" alt="License"></a>
  <a href="https://github.com/digyear/ZeroAny-Workbench/stargazers"><img src="https://img.shields.io/github/stars/digyear/ZeroAny-Workbench?style=flat-square&color=f5a623" alt="Stars"></a>
  <img src="https://img.shields.io/badge/built%20with-Rust%20%2B%20Tauri-CE422B?style=flat-square" alt="Rust + Tauri">
  <img src="https://img.shields.io/badge/binary-~25%20MB-4f94d4?style=flat-square" alt="~20 MB">
  <img src="https://img.shields.io/badge/MCP-built--in-1dc880?style=flat-square" alt="MCP built-in">

  

</p>

<p align="center">
  <a href="https://github.com/digyear/ZeroAny-Workbench">Project</a> ·
  <a href="https://github.com/digyear/ZeroAny-Workbench/releases/latest"><strong>Download →</strong></a> ·
  <a href="https://github.com/digyear/ZeroAny-Workbench/issues">Report a bug</a>
</p>

<p align="center">
<b>ZeroAny Workbench has no associated cryptocurrency or token. Any tokens using our name are scams </b>
</p>

---

> [!IMPORTANT]
> **ZeroAny Workbench is an independent, open-source developer product under the
> ZeroAny brand.** The project is licensed under the Apache License 2.0.

---

You've been flipping between multiple apps to do one job. ZeroAny Workbench runs all of them in one shell — coding agents, an API client, SQL and NoSQL editors, SSH sessions, a remote file browser, and a project workspace — and gives each mode its own AI, tuned for that workflow. Every mode is one keystroke away. Your data stays on your machine.

---

## Modes

| | Mode | What it does | Key capabilities |
|---|---|---|---|
| `01` | **Agent** | Run multiple coding agents in parallel — and bring existing local sessions into one searchable center. | Claude · Codex · Gemini · OpenCode · Hermes profiles · global Session Center · isolated git worktrees · reliable session resume · Context Manager · per-session git identity · usage analytics |
| `02` | **Workspace** | Boards + notes your agents can read, write, and act on. | Kanban boards · markdown notes · AI coworkers · GitHub & GitLab issue import · full MCP integration |
| `03` | **REST** | An API client your AI — and any external agent — can drive. | Collections · environments · AI batch runner · MCP-exposed (agents create / read / update / delete collections and requests) |
| `04` | **SQL** | One client, every engine. | PostgreSQL · MySQL · ClickHouse · SQLite · Cloudflare D1 · schema-aware AI · cross-dialect translation · SSH tunnels shared with other modes |
| `05` | **NoSQL** | Document and key-value stores, side by side. | MongoDB · Redis · aggregation pipeline builder · interactive Redis console · engine-aware AI |
| `06` | **SSH** | A terminal with an AI co-pilot. | Profiles · multi-tab per host · port forwarding · keychain-backed creds · **two AI modes**: permission-gated and auto |
| `07` | **Explorer** | Every storage, one browser. | Local FS · S3 (and S3-compatibles) · Azure Blob · SFTP · FTP · drag-and-drop transfers · AI scan |
| `08` | **Atlas** | Drag, resize, and snap your open tabs into a free-form spatial workspace. | Pan / zoom canvas · per-workspace layouts · Atlas-spawned shells · hosts Agent, SSH, REST, SQL, NoSQL, and Explorer tabs |

Plus a cross-mode **History** layer — a queryable log of every session, request, query, and command across modes.

---

## Inside each mode

### Agent

Spawn coding agents in parallel — **Claude**, **Codex**, **Gemini**, **OpenCode**, and **Hermes Agent** — without ever leaving the editor. Each managed session is independent: its own purpose, git worktree, context, model, and provider-native session identity.

Per-session controls:

- **Purpose** — pin the session's intent up front (see table below)
- **Provider** — choose Claude, Codex, Gemini, OpenCode, or Hermes for each session
- **Hermes profile** — choose the default or a named Hermes profile and keep that identity when the session is reopened
- **Base branch + worktree branch** — start from the repository's current branch by default, then edit either value before creating an isolated worktree
- **Git identity** — commit as a coworker, not yourself
- **Skip permissions** — for fully-autonomous runs when you trust the prompt
- **Context injection** — pin files, folders, or MCP servers via the **Context Manager** (save sets, reuse across sessions)
- **Plugins** — load extra tools from the **Plugin Manager**
- **Resume** — reopen the provider's original conversation instead of silently starting a new one

#### Session Center

ZeroAny Workbench does not limit you to sessions created inside the app. The global **Session Center** discovers existing local conversations from supported agent CLIs and turns them into one searchable catalog.

- **Cross-provider discovery** — scan Claude, Codex, Gemini, OpenCode, and every active Hermes profile from one view
- **Project-aware grouping** — worktree conversations are grouped under their canonical main repository instead of appearing as unrelated projects
- **Search and filters** — filter by provider, project, date range, and status; search titles, previews, paths, native session IDs, and Hermes profile names
- **Profile-safe identity** — identical Hermes session IDs in different profiles remain separate, and adopted sessions resume with their original profile
- **Adopt or hide** — open an external conversation as a managed ZeroAny Workbench session, or hide catalog entries without deleting provider-owned data
- **Fault-tolerant refresh** — one unreadable Hermes profile cannot prevent other profiles and providers from being discovered
- **Resume fidelity** — preserve native session IDs through Hermes compression and immediate same-process reopen, so continuing a conversation does not accidentally create a fresh session

#### Purposes

Every ZeroAny Workbench session has a **purpose** that shapes the agent's focus from the first message. No more prompting it to "act like a code reviewer" mid-conversation.

| Purpose | What the agent focuses on |
|---|---|
| **Brainstorming** | Architecture, tradeoffs, multiple approaches — before writing a line |
| **Development** | Clean, tested, pattern-consistent code shipped in small verified steps |
| **Code Review** | Bugs, security holes, edge cases — with file and line references |
| **PR Review** | End-to-end pull-request analysis: what changed, what's good, what needs work |
| **Debugging** | Root cause, not band-aids — reproduce, trace, verify the fix actually works |
| **Custom** | Resume an existing provider session or define your own mode |

Every session emits **usage analytics** — tokens in / out, cost, cache hit, model mix — per session and aggregated across your day.

### Workspace

A shared layer your agents can read, write, and act on through MCP.

- **Kanban boards** — Backlog → Todo → In Progress → In Review → Done. *Review* is a safety gate, not a column you skip past.
- **Two-way Git integration** — pull open issues from **GitHub** and **GitLab** into a board, push finished cards back as pull requests.
- **Project linking** — point a workspace at a git repo and ZeroAny Workbench auto-spawns one board per subproject.
- **Notion-style notes** — a real WYSIWYG markdown editor, not a textarea. Per-project, linked to sessions, exposed through MCP.
- **AI coworkers** — named personas (Tech Lead, Brainstormer, Developer, Reviewer, QA) with their own prompts and providers. They read cards, comment, request changes, claim work, commit, and raise PRs.
- **Single-owner lock** — only one coworker (or one terminal session) owns a card at any moment. Switching owners, claiming from a terminal, and chatting in the card drawer all go through the same lock — no race conditions, no lost work.
- **Pinned Inbox** — mentions, review requests, and approval queues across every board, in one pane.
- **Cross-board search** — instant full-text search across every note and card.
- **Default-on MCP** — boards, cards, notes, and `@-mention a coworker` are all MCP tools an external agent can call. Auto-starts when the app launches — no flag, no setup.

### REST

An API client driven by AI and exposed over MCP.

- **MCP integration** — external agents can list, read, create, update, and delete collections and requests.
- **AI assistance** — describe an endpoint and the AI writes the request; ask *"run the smoke tests in staging"* and the AI fires the whole collection.
- **Execution reports** — pass / fail summary with the failing request inline.
- Collections, environments, request history, Postman v2 / v2.1 import.

### SQL

One editor, multiple engines, schema-aware AI.

- **Engines** — PostgreSQL · MySQL · ClickHouse · SQLite · Cloudflare D1
- **AI assistance** — natural language → schema-aware SQL, ready to run
- **Cross-dialect translation** — Postgres → MySQL → ClickHouse; AI rewrites the query for the target engine
- **SSH tunneling** shared with other modes — wire your bastion once, all modes use it

### NoSQL

Document stores and key-value engines, side by side.

- **MongoDB** — collections, JSON query editor, stage-by-stage aggregation pipeline builder with previews
- **Redis** — keys, TTL, streams, pub-sub, interactive console
- **Engine-aware AI** — never confuses a `find` with a `SCAN`

### SSH

Persistent SSH with two AI modes:

- **Permission mode** — every command the AI proposes is gated; you approve before it touches the wire.
- **Auto mode** — describe what you need; the AI executes and streams the output back.

Plus: reusable profiles, **multiple tabs against the same host** (no re-authenticating), port forwarding, ed25519 and agent forwarding, keychain-backed credentials.

### Explorer

Every backend, one browser, one set of shortcuts.

- **Backends** — Local FS · Amazon S3 (and S3-compatibles like R2, MinIO, Wasabi) · Azure Blob · SFTP · FTP
- **One-click presets** — pre-configured profiles for the common S3-compatible providers, so connecting takes seconds
- **Drag-and-drop transfers** — drop files into any backend, right-click "Download to…", with a live transfer panel showing progress and cancel
- **AI assistance** — *"what grew today?"*, *"find images larger than 5 MB"* — natural-language file ops
- **Inline preview** — text, JSON, CSV without a round-trip download

### Atlas

A free-form spatial workspace where every open tab becomes a draggable window. Pan, zoom, snap to neighbors, and stop alt-tabbing.

- **Universal tile** — Agent sessions, SSH terminals, REST requests, SQL editors, NoSQL queries, Explorer file browsers, and Atlas-spawned shells all coexist as resizable windows on one canvas.
- **State preserved across modes** — switching to Atlas and back doesn't reset anything; CodeMirror undo stacks, scroll positions, terminal scrollback, and connection state survive the round-trip.
- **In-tile control surfaces** — SQL tiles get a Run button + results table + connection picker; NoSQL tiles get a connection/collection picker; REST tiles get the full headers/auth/params/body editor + env picker + Send.
- **Snap guides + per-workspace layouts** — tiles snap to the edges of neighbors while dragging; tile positions are persisted per workspace, so each project has its own canvas memory.
- **Spawn shells anywhere** — a shell-spawn button drops a fresh terminal onto the canvas, scoped to that workspace.

---

## What makes ZeroAny Workbench different

**One window, not many.** Every developer's day is fragmented across a code editor, a REST client, a SQL GUI, a Mongo shell, a terminal, and a project board. ZeroAny Workbench collapses all of them into one shell with shared sessions, shared SSH tunnels, and shared AI context — switch modes with one keystroke.

**An AI per workflow, not a generic chat.** REST's AI understands your collections. SQL's AI knows your schema. SSH's AI refuses destructive commands without confirmation. The Workspace AI moves cards, leaves comments, and raises pull requests. Each one is tuned for what it sits next to — not a single chatbot bolted onto the side.

**Your existing agent history, not another empty inbox.** Session Center indexes conversations already stored by your local agent CLIs, groups worktree sessions under the real project, and lets you resume or adopt them without copying provider-owned data into a proprietary cloud.

**Built-in MCP server, not a plug-in.** ZeroAny Workbench runs an MCP server out of the box, exposing 45+ tools across boards, cards, notes, REST collections, and coworker coordination. Claude Desktop, Cursor, Cline, Continue, or any MCP-speaking client can read, edit, and add to your workspace from the outside.

**Local-first by default.** Your sessions, your notes, your queries, your keys live on your disk. The desktop app is the source of truth. Sync is opt-in, per-feature.

**Native.** Rust + Tauri. ~20 MB binary. Sub-second cold start. No Electron tax.

---

## MCP

ZeroAny Workbench ships an MCP server with **45+ tools** — `boards_*`, `cards_*`, `notes_*`, `rest_collection_*`, `coworkers_*`, `workspace_*`, `activity_feed`, and `cards_call_coworker` (so the agent in your terminal can `@-mention` a coworker on a card without leaving the shell). The server **auto-starts on launch** — no flag, no extra setup. Any MCP-speaking client can drive your workspace.

### From Claude Desktop

Add to your `claude_desktop_config.json`:

```jsonc
{
  "mcpServers": {
    "zeroany-workbench": {
      "command": "zeroany-workbench",
      "args": ["mcp", "serve", "--stdio"]
    }
  }
}
```

### From Cursor / Cline / Continue

Point your MCP client at the local ZeroAny Workbench process (`stdio` or `http://localhost:7421/mcp`). The agent can now list boards, create cards, append notes, raise PRs, search REST collections, and coordinate with the named coworkers you defined inside the app — all without leaving its own UI.

---

## AI assistance — bring your own key, or use ZeroAny AI

Every mode's AI runs on the provider you choose.

| Option | How it works |
|---|---|
| **BYOK** | Drop in your own Anthropic, OpenAI, Google, or OpenCode key — ZeroAny Workbench talks directly to the provider. No middleman. |
| **ZeroAny AI credits** | Use managed AI credits across every mode and every provider — no separate billing. |

Set keys in **Settings → AI Providers**. Per-mode model choice; per-session model override.

## Cloud sync

Opt-in, encrypted.Agent Context, Co-Worker Profil, REST collections,SQL/NoSQL and SSH profiles sync across machines — or stay local, your call. Sessions and credentials never leave the device unless you explicitly enable sync for them.

---

## Built with

- **Frontend** — SvelteKit + Svelte 5
- **Native shell** — Tauri v2 (Rust)
- **Persistence** — SQLite (local), optional encrypted cloud sync
- **Terminals** — `xterm.js` + cross-platform PTY
- **MCP** — built-in JSON-RPC server, stdio and HTTP transports

---

## Star History

<a href="https://www.star-history.com/?repos=digyear%2FZeroAny-Workbench&type=date&legend=bottom-right">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=digyear/ZeroAny-Workbench&type=date&theme=dark&legend=top-left" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=digyear/ZeroAny-Workbench&type=date&legend=top-left" />
   <img alt="Star History Chart" src="https://api.star-history.com/chart?repos=digyear/ZeroAny-Workbench&type=date&legend=top-left" />
 </picture>
</a>

---

## License

This project is licensed under the [Apache License 2.0](LICENSE).

Contributions are welcome under the terms of the [Apache License 2.0](LICENSE).

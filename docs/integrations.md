# ICM Integrations

ICM integrates with **16 AI tools** across 4 integration modes: MCP server, CLI instructions, skills/rules, and hooks.

## Quick Setup

```bash
# Install everything for all detected tools
icm init --mode all

# Or pick a specific mode
icm init --mode mcp      # MCP server configs (default)
icm init --mode cli      # Inject instructions into CLAUDE.md, AGENTS.md, etc.
icm init --mode skill    # Slash commands & rules
icm init --mode hook     # Claude Code hooks + OpenCode plugin
```

## Supported Tools

### Editors & IDEs

#### Claude Code

```bash
# Option 1: MCP (recommended)
claude mcp add icm -- icm serve

# Option 2: Full setup (MCP + hooks + skills)
icm init --mode all
```

**Config:** `~/.claude.json` → `mcpServers.icm`

**Hooks (optional):**

| Hook | Event | What it does |
|------|-------|-------------|
| `icm hook pre` | PreToolUse | Auto-allow `icm` commands |
| `icm hook post` | PostToolUse | Extract facts every 15 calls |
| `icm hook compact` | PreCompact | Extract before context compression |
| `icm hook prompt` | UserPromptSubmit | Inject recalled context |

**Skills:** `/recall`, `/remember` slash commands.

---

#### Claude Desktop

```bash
icm init --mode mcp
```

**Config:** `~/Library/Application Support/Claude/claude_desktop_config.json` → `mcpServers.icm`

---

#### Cursor

```bash
icm init --mode mcp
icm init --mode skill   # Installs ~/.cursor/rules/icm.mdc
```

**MCP Config:** `~/.cursor/mcp.json` → `mcpServers.icm`

**Rule file:** `~/.cursor/rules/icm.mdc` — Always-on rule that instructs Cursor to use ICM for persistent memory.

---

#### Windsurf

```bash
icm init --mode mcp     # MCP server
icm init --mode cli     # Injects instructions into .windsurfrules
```

**MCP Config:** `~/.codeium/windsurf/mcp_config.json` → `mcpServers.icm`

**Rules:** `.windsurfrules` in project root — project-scoped instructions.

---

#### VS Code / GitHub Copilot

```bash
icm init --mode mcp     # MCP server for VS Code
icm init --mode cli     # Injects instructions into .github/copilot-instructions.md
```

**MCP Config:** `~/Library/Application Support/Code/User/mcp.json` → `servers.icm`

> Note: VS Code uses `"servers"` as the JSON key, not `"mcpServers"`.

**Copilot instructions:** `.github/copilot-instructions.md` — automatically loaded by GitHub Copilot at session start. Contains ICM recall/store instructions.

---

#### Zed

```bash
icm init --mode mcp
```

**Config:** `~/.zed/settings.json` → `context_servers.icm`

Zed uses a nested format:
```json
{
  "context_servers": {
    "icm": {
      "command": { "path": "/path/to/icm", "args": ["serve"] }
    }
  }
}
```

---

### Terminal Tools

#### Amp

```bash
icm init --mode mcp
icm init --mode skill   # Installs /icm-recall and /icm-remember
```

**Config:** `~/.config/amp/settings.json` → `amp.mcpServers.icm`

**Skills:** `~/.config/amp/skills/icm-recall.md`, `icm-remember.md`

---

#### Amazon Q

```bash
icm init --mode mcp
```

**Config:** `~/.aws/amazonq/mcp.json` → `mcpServers.icm`

---

#### OpenAI Codex CLI

```bash
icm init --mode mcp     # TOML config
icm init --mode cli     # Injects into AGENTS.md
```

**Config:** `~/.codex/config.toml`

```toml
[mcp_servers.icm]
command = "/path/to/icm"
args = ["serve"]
```

**Instructions:** `AGENTS.md` in project root.

---

#### Gemini Code Assist

```bash
icm init --mode mcp
icm init --mode cli     # Injects into ~/.gemini/GEMINI.md
```

**Config:** `~/.gemini/settings.json` → `mcpServers.icm`

**Instructions:** `~/.gemini/GEMINI.md`

---

#### OpenCode

```bash
icm init --mode mcp     # MCP server config
icm init --mode hook    # Installs JS plugin with hooks
```

**MCP Config:** `~/.config/opencode/opencode.json` → `mcp.icm`

**Plugin:** `~/.config/opencode/plugins/icm.js`

| Event | What it does |
|-------|-------------|
| `tool.execute.after` | Extract facts from tool output |
| `experimental.session.compacting` | Extract before context compression |
| `session.created` | Recall context at session start |

---

### VS Code Extensions

#### Cline

```bash
icm init --mode mcp
```

**Config:** `~/Library/Application Support/Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json`

---

#### Roo Code

```bash
icm init --mode mcp
icm init --mode skill   # Installs ~/.roo/rules/icm.md
```

**Config:** `~/Library/Application Support/Code/User/globalStorage/rooveterinaryinc.roo-cline/settings/mcp_settings.json`

**Rule:** `~/.roo/rules/icm.md`

---

#### Kilo Code

```bash
icm init --mode mcp
```

**Config:** `~/Library/Application Support/Code/User/globalStorage/kilocode.kilo-code/settings/mcp_settings.json`

---

## Integration Modes Summary

| Mode | What it does | Tools |
|------|-------------|-------|
| `mcp` | Configures MCP server in each tool's config | All 14 tools |
| `cli` | Injects ICM instructions into instruction files | Claude Code, Codex, Gemini, Copilot, Windsurf |
| `skill` | Creates slash commands and rule files | Claude Code, Cursor, Roo Code, Amp |
| `hook` | Installs hooks/plugins for automatic extraction | Claude Code (4 hooks), OpenCode (JS plugin) |

## Manual Setup

For any MCP-compatible tool not listed above:

```json
{
  "command": "icm",
  "args": ["serve"],
  "env": {}
}
```

Or with compact mode (shorter responses, fewer tokens):

```json
{
  "command": "icm",
  "args": ["serve", "--compact"],
  "env": {}
}
```

## MCP Tools Available

When connected via MCP, these 18 tools are available:

| Tool | Description |
|------|-------------|
| `icm_memory_store` | Store a memory |
| `icm_memory_recall` | Search memories (hybrid: FTS + vector) |
| `icm_memory_update` | Update an existing memory |
| `icm_memory_forget` | Delete a memory |
| `icm_memory_consolidate` | Merge topic memories into one |
| `icm_memory_health` | Topic hygiene audit |
| `icm_memory_embed_all` | Generate embeddings for all memories |
| `icm_memory_extract_patterns` | Detect recurring patterns |
| `icm_memory_list_topics` | List all topics with stats |
| `icm_memory_stats` | Global statistics |
| `icm_memoir_create` | Create a knowledge graph |
| `icm_memoir_show` | Display memoir structure |
| `icm_memoir_add_concept` | Add concept to memoir |
| `icm_memoir_link` | Link concepts |
| `icm_memoir_search` | Search within a memoir |
| `icm_memoir_search_all` | Search across all memoirs |
| `icm_feedback_record` | Record a correction |
| `icm_feedback_search` | Search past corrections |

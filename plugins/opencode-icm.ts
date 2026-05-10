// ICM auto-extraction + auto-recall plugin for OpenCode
// Installed by `icm init --mode hook`
//
// Layer 0: tool.execute.after                       -> extract facts from tool output
// Layer 1: experimental.session.compacting          -> extract from conversation before compaction
// Layer 2: session.created                          -> log on session start (no API to inject from here)
// Layer 3: experimental.chat.system.transform       -> inject recalled context into the system prompt
//
// Issue #169: the previous plugin only logged on session.created and never
// actually injected anything into the conversation. OpenCode's hook API
// exposes `experimental.chat.system.transform`, which lets us push extra
// system-prompt strings before each model call — that's the correct place
// to deliver recalled context into the agent's context window.

import type { Plugin } from "@opencode-ai/plugin"
import { execFileSync } from "child_process"

const EXTRACT_EVERY = 3
let toolCallCount = 0

function icmExtract(args: string[], input: string): void {
  try {
    execFileSync("icm", args, {
      encoding: "utf-8",
      timeout: 10000,
      input,
      stdio: ["pipe", "pipe", "pipe"],
    })
  } catch {
    // silent — extraction is best-effort
  }
}

/// Capture stdout of `icm <args>` synchronously. Returns the empty string on
/// any failure so a missing/old binary or empty memory store can never break
/// a chat turn.
function icmCapture(args: string[]): string {
  try {
    const out = execFileSync("icm", args, {
      encoding: "utf-8",
      timeout: 10000,
      stdio: ["ignore", "pipe", "pipe"],
    })
    return String(out).trim()
  } catch {
    return ""
  }
}

export const IcmPlugin: Plugin = async ({ $, directory }) => {
  const project = directory?.split("/").pop() || "project"

  // Verify icm binary is available
  try {
    const v = await $`icm --version`.quiet().nothrow()
    const version = String(v.stdout).trim()
    if (!version) throw new Error("not found")
    console.error(`[icm] plugin loaded (${version})`)
  } catch {
    console.warn("[icm] icm binary not found in PATH — plugin disabled")
    return {}
  }

  // De-duplicate per-session to avoid re-injecting the same wake-up pack
  // on every model turn. The transform hook fires per chat call; we only
  // want context once at the start of each session.
  const injectedSessions = new Set<string>()

  return {
    // Layer 0: extract facts from tool output every N calls
    "tool.execute.after": async (input: any, result: any) => {
      const tool = String(input?.tool ?? "")
      if (!tool || tool.startsWith("icm") || tool.startsWith("mcp__icm__")) return

      toolCallCount++
      if (toolCallCount < EXTRACT_EVERY) return
      toolCallCount = 0

      // OpenCode puts tool output in result.metadata.output
      const output =
        result?.metadata?.output ?? result?.output ?? (typeof result === "string" ? result : "")
      if (!output || typeof output !== "string" || output.length < 20) return

      try {
        icmExtract(["extract", "--store-raw", "-p", project], output.slice(0, 4000))
      } catch {
        // silent
      }
    },

    // Layer 1: extract from conversation before compaction
    "experimental.session.compacting": async ({ messages }: any) => {
      if (!messages || !Array.isArray(messages)) return

      const text = messages
        .filter((m: any) => m.role === "assistant")
        .slice(-20)
        .map((m: any) => {
          if (typeof m.content === "string") return m.content
          if (Array.isArray(m.content))
            return m.content
              .filter((p: any) => p.type === "text")
              .map((p: any) => p.text)
              .join("\n")
          return ""
        })
        .join("\n")
        .slice(-4000)

      if (text.length < 50) return

      try {
        icmExtract(["extract", "--store-raw", "-p", project], text)
      } catch {
        // silent
      }
    },

    // Layer 2: log on session creation. OpenCode's `session.created` hook
    // is observational — there is no API to inject context from here.
    // Real injection happens in `experimental.chat.system.transform` below.
    "session.created": async () => {
      console.error(`[icm] session ready, will inject project context on first chat turn`)
    },

    // Layer 3: inject recalled context into the system prompt.
    //
    // Issue #169: this is the hook the previous plugin was missing. It
    // fires before each model call; we append the project's wake-up pack
    // and a project-scoped recall to `output.system`, which OpenCode
    // concatenates into the LLM's system prompt.
    //
    // Run only once per session (keyed by sessionID) so we don't re-inject
    // the same context on every turn.
    "experimental.chat.system.transform": async (input: any, output: any) => {
      const sessionID = input?.sessionID ?? "no-session"
      if (injectedSessions.has(sessionID)) return
      injectedSessions.add(sessionID)

      // Wake-up pack: critical/high-importance facts + preferences.
      const wakeUp = icmCapture(["wake-up", "--project", project])
      if (wakeUp) {
        output.system.push(wakeUp)
      }

      // Project-scoped recall: top-N relevant memories for this project.
      const ctx = icmCapture(["recall-project", "--limit", "5"])
      if (ctx) {
        output.system.push(ctx)
        console.error(
          `[icm] injected ${ctx.split("\n").length} lines of project context into system prompt`,
        )
      }
    },
  }
}

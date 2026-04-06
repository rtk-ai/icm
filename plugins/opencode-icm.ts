// ICM auto-extraction plugin for OpenCode
// Installed by `icm init --mode hook`
//
// Layer 0: tool.execute.after → extract facts from tool output
// Layer 1: experimental.session.compacting → extract from conversation before compaction
// Layer 2: session.created → inject recalled context

import type { Plugin } from "@opencode-ai/plugin"

const EXTRACT_EVERY = 3
let toolCallCount = 0

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

  return {
    // Layer 0: extract facts from tool output every N calls
    "tool.execute.after": async ({ tool, output }) => {
      if (!tool || tool.startsWith("icm") || tool.startsWith("mcp__icm__")) return

      toolCallCount++
      if (toolCallCount < EXTRACT_EVERY) return
      toolCallCount = 0

      if (!output || typeof output !== "string" || output.length < 20) return

      try {
        const text = output.slice(0, 4000)
        await $`echo ${text} | icm extract --store-raw -p ${project}`.quiet().nothrow()
      } catch {
        // silent
      }
    },

    // Layer 1: extract from conversation before compaction
    "experimental.session.compacting": async ({ messages }) => {
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
        await $`echo ${text} | icm extract --store-raw -p ${project}`.quiet().nothrow()
      } catch {
        // silent
      }
    },

    // Layer 2: recall context at session start
    "session.created": async () => {
      try {
        const result = await $`icm recall-context ${project} --limit 5`.quiet().nothrow()
        const ctx = String(result.stdout).trim()
        if (ctx) {
          console.error(`[icm] recalled ${ctx.split("\n").length} lines of context`)
        }
      } catch {
        // silent
      }
    },
  }
}

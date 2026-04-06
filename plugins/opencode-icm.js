// ICM auto-extraction plugin for OpenCode
// Installed by `icm init --mode hook`
//
// Layer 0: tool.execute.after → extract facts from tool output
// Layer 1: experimental.session.compacting → extract from conversation before compaction
// Layer 2: session.created → inject recalled context

import { execFileSync, execSync } from "child_process";

const ICM_BIN = process.env.ICM_BIN || "icm";
let toolCallCount = 0;
const EXTRACT_EVERY = 10;

function icm(...args) {
  try {
    return execFileSync(ICM_BIN, args, {
      encoding: "utf-8",
      timeout: 10000,
      stdio: ["pipe", "pipe", "pipe"],
    }).trim();
  } catch {
    return "";
  }
}

// Pass text via stdin to avoid shell escaping issues
function icmWithStdin(args, input) {
  try {
    return execFileSync(ICM_BIN, args, {
      encoding: "utf-8",
      timeout: 10000,
      input: input,
      stdio: ["pipe", "pipe", "pipe"],
    }).trim();
  } catch {
    return "";
  }
}

function getProject(directory) {
  if (!directory) return "project";
  return directory.split("/").pop() || "project";
}

export const IcmPlugin = async ({ directory }) => {
  const project = getProject(directory);

  // Verify icm binary is available
  const version = icm("--version");
  if (!version) {
    console.error("[icm] icm binary not found in PATH — plugin disabled");
    return {};
  }
  console.error(`[icm] plugin loaded (${version})`);

  return {
    // Layer 0: extract facts from tool output every N calls
    "tool.execute.after": async ({ tool, output }) => {
      // Skip ICM's own tools
      if (!tool || tool.startsWith("icm") || tool.startsWith("mcp__icm__")) return;

      toolCallCount++;
      if (toolCallCount < EXTRACT_EVERY) return;
      toolCallCount = 0;

      if (!output || typeof output !== "string" || output.length < 20) return;

      try {
        // Use stdin instead of -t flag to avoid shell escaping issues
        icmWithStdin(["extract", "--store-raw", "-p", project], output.slice(0, 4000));
      } catch {
        // silent
      }
    },

    // Layer 1: extract from conversation before compaction
    "experimental.session.compacting": async ({ messages }) => {
      if (!messages || !Array.isArray(messages)) return;

      // Collect assistant text from recent messages
      const text = messages
        .filter((m) => m.role === "assistant")
        .slice(-20)
        .map((m) => {
          if (typeof m.content === "string") return m.content;
          if (Array.isArray(m.content))
            return m.content
              .filter((p) => p.type === "text")
              .map((p) => p.text)
              .join("\n");
          return "";
        })
        .join("\n")
        .slice(-4000);

      if (text.length < 50) return;

      try {
        // Use stdin instead of -t flag to avoid shell escaping issues
        icmWithStdin(["extract", "--store-raw", "-p", project], text);
      } catch {
        // silent
      }
    },

    // Layer 2: recall context at session start
    "session.created": async () => {
      try {
        const ctx = icm("recall-context", project, "--limit", "5");
        if (ctx) {
          console.error(`[icm] recalled ${ctx.split("\n").length} lines of context`);
        }
      } catch {
        // silent
      }
    },
  };
};

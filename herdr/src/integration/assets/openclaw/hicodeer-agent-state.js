// installed by HiCodeer
// managed by herdr; reinstalling or updating the integration overwrites this file.
// HERDR_INTEGRATION_ID=openclaw
// HERDR_INTEGRATION_VERSION=2

import net from "node:net";
import { definePluginEntry } from "openclaw/plugin-sdk/plugin-entry";

const SOURCE = "hicodeer:openclaw";
const AGENT = "openclaw";
let reportSeq = Date.now() * 1000;

function nextReportSeq() {
  reportSeq += 1;
  return reportSeq;
}

function reportingEnabled() {
  return Boolean(
    process.env.HERDR_ENV === "1" &&
      process.env.HICODEER_AGENT_SOCKET_PATH &&
      process.env.HERDR_PANE_ID,
  );
}

function request(method, params) {
  const paneId = process.env.HERDR_PANE_ID;
  const socketPath = process.env.HICODEER_AGENT_SOCKET_PATH;

  if (!paneId || !socketPath) {
    return Promise.resolve();
  }

  const socketEndpoint =
    process.platform === "win32" ? `\\\\.\\pipe\\${socketPath}` : socketPath;
  const requestId = `${SOURCE}:${Date.now()}:${Math.floor(Math.random() * 1_000_000)
    .toString()
    .padStart(6, "0")}`;
  const message = {
    id: requestId,
    method,
    params: {
      pane_id: paneId,
      source: SOURCE,
      agent: AGENT,
      seq: nextReportSeq(),
      ...params,
    },
  };

  return new Promise((resolve) => {
    const client = net.createConnection(socketEndpoint, () => {
      client.write(`${JSON.stringify(message)}\n`);
    });

    const finish = () => {
      client.destroy();
      resolve();
    };

    client.setTimeout(500, finish);
    client.on("data", finish);
    client.on("error", finish);
    client.on("end", finish);
    client.on("close", resolve);
  });
}

function sessionIDFrom(event) {
  const value = event?.sessionId ?? event?.sessionID ?? event?.session?.id;
  return typeof value === "string" && value ? value : undefined;
}

function reportState(state, params = {}) {
  return request("pane.report_agent", { state, ...params });
}

function textFromContent(content) {
  if (typeof content === "string") {
    return content.trim();
  }
  if (!Array.isArray(content)) {
    return "";
  }
  return content
    .map((block) => (typeof block === "string" ? block : block?.text ?? ""))
    .filter((text) => typeof text === "string" && text.trim())
    .join("\n")
    .trim();
}

// The final assistant message of the run, which is what a finished turn shows.
// `willRetry` runs continue, so their partial output is not a finished reply.
function lastAssistantMessage(messages) {
  if (!Array.isArray(messages)) {
    return undefined;
  }
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (message?.role !== "assistant") {
      continue;
    }
    const text = textFromContent(message.content);
    if (text) {
      return text;
    }
  }
  return undefined;
}

function registerHook(api, hookName, handler) {
  try {
    api.on(hookName, handler);
  } catch {
    // An older host may not know this hook; the others still report.
  }
}

export default definePluginEntry({
  id: "hicodeer-agent-state",
  name: "HiCodeer Agent State",
  description: "Reports OpenClaw lifecycle state to HiCodeer.",
  register(api) {
    if (!reportingEnabled()) {
      return;
    }

    registerHook(api, "session_start", (event) => {
      const sessionID = sessionIDFrom(event);
      if (!sessionID) {
        return undefined;
      }
      return request("pane.report_agent_session", {
        agent_session_id: sessionID,
        session_start_source: "startup",
      });
    });

    registerHook(api, "before_agent_run", (event) =>
      reportState("working", sessionIDFrom(event) ? { agent_session_id: sessionIDFrom(event) } : {}),
    );

    registerHook(api, "before_tool_call", (event) =>
      reportState("working", sessionIDFrom(event) ? { agent_session_id: sessionIDFrom(event) } : {}),
    );

    registerHook(api, "agent_end", (event) => {
      if (event?.willRetry) {
        return undefined;
      }
      const params = {};
      const sessionID = sessionIDFrom(event);
      if (sessionID) {
        params.agent_session_id = sessionID;
      }
      const reply = lastAssistantMessage(event?.messages);
      if (reply) {
        params.message = reply;
      }
      return reportState("idle", params);
    });
  },
});

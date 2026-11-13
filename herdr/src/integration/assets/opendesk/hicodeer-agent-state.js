// installed by HiCodeer
// managed by HiCodeer; reinstalling or updating the integration overwrites this file.
// add custom hooks beside this file instead of editing it.
// HICODEER_INTEGRATION_ID=opendesk
// HICODEER_INTEGRATION_VERSION=1

import net from "node:net";
import { definePlugin } from "@bitclub.ai/opendesk-plugin-sdk";

const SOURCE = "hicodeer:opendesk";
const AGENT = "opendesk";
let reportSeq = Date.now() * 1000;

function nextReportSeq() {
  reportSeq += 1;
  return reportSeq;
}

function reportingEnabled() {
  return Boolean(
    process.env.HICODEER_ENV === "1" &&
      process.env.HICODEER_AGENT_SOCKET_PATH &&
      process.env.HICODEER_PANE_ID,
  );
}

function request(method, params) {
  const paneId = process.env.HICODEER_PANE_ID;
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

function textFrom(value) {
  if (typeof value === "string") {
    return value.trim();
  }
  if (Array.isArray(value)) {
    return value
      .map((part) => (typeof part === "string" ? part : part?.text ?? ""))
      .filter((text) => typeof text === "string" && text.trim())
      .join("\n")
      .trim();
  }
  return "";
}

function lastAssistantMessage(messages) {
  if (!Array.isArray(messages)) {
    return undefined;
  }
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (message?.role !== "assistant") {
      continue;
    }
    const text = textFrom(message.content);
    if (text) {
      return text;
    }
  }
  return undefined;
}

// A permission prompt parks the turn on the user, so it states what is being
// asked instead of the reply of the previous turn. The SDK payload is a
// `PermissionRequest` with the action and the path it applies to.
function describeQuestion(input) {
  const action = textFrom(input?.action);
  const path = textFrom(input?.resourcePath);
  if (action && path) {
    return `${action}: ${path}`;
  }
  for (const candidate of [action, path, input?.question, input?.description]) {
    const text = textFrom(candidate);
    if (text) {
      return text;
    }
  }
  return undefined;
}

function registerHook(context, hookName, handler) {
  try {
    context.hooks.on(hookName, handler);
  } catch {
    // An older host may not know this hook; the others still report.
  }
}

export default definePlugin({
  id: "hicodeer-agent-state",
  setup(context) {
    if (!reportingEnabled()) {
      return;
    }

    registerHook(context, "chat.params", () => request("pane.report_agent", { state: "working" }));

    registerHook(context, "tool.execute.after", () =>
      request("pane.report_agent", { state: "working" }),
    );

    registerHook(context, "permission.ask", (input) => {
      const params = { state: "blocked" };
      const question = describeQuestion(input);
      if (question) {
        params.message = question;
      }
      return request("pane.report_agent", params);
    });

    // `task.review` carries the turn's messages; the run is over by then, so
    // the final assistant message becomes the reply the panel shows.
    registerHook(context, "task.review", (input) => {
      const params = { state: "idle" };
      const reply = lastAssistantMessage(input?.messages);
      if (reply) {
        params.message = reply;
      }
      return request("pane.report_agent", params);
    });
  },
});

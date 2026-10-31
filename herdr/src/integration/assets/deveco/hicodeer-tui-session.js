// installed by HiCodeer
// managed by herdr; reinstalling or updating the integration overwrites this file.
// add custom hooks/plugins beside this file instead of editing it.
// HERDR_INTEGRATION_ID=deveco-tui
// HERDR_INTEGRATION_VERSION=2

import net from "node:net";

const SOURCE = "hicodeer:deveco";
const AGENT = "deveco";
const POLL_INTERVAL_MS = 250;
const SOCKET_TIMEOUT_MS = 500;

// DevEco Code exposes the selected route and per-session status through its TUI
// plugin API, so this reporter polls those two values instead of deriving state
// from server events the TUI does not forward to plugins. The table mirrors the
// one the server-side plugin uses: both read the same `session.status` values.
const STATE_BY_SESSION_STATUS = new Map([
  ["active", "working"],
  ["busy", "working"],
  ["pending", "working"],
  ["retry", "working"],
  ["running", "working"],
  ["streaming", "working"],
  ["working", "working"],
  ["idle", "idle"],
]);

let reportSeq = Date.now() * 1000;

function nextReportSeq() {
  reportSeq += 1;
  return reportSeq;
}

function requestOnce(method, params) {
  const paneId = process.env.HERDR_PANE_ID;
  const socketPath = process.env.HICODEER_AGENT_SOCKET_PATH;

  if (!paneId || !socketPath) {
    return Promise.resolve(false);
  }

  const socketEndpoint =
    process.platform === "win32" ? `\\\\.\\pipe\\${socketPath}` : socketPath;

  const request = {
    id: `${SOURCE}:tui:${Date.now()}:${Math.floor(Math.random() * 1_000_000)
      .toString()
      .padStart(6, "0")}`,
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
    let settled = false;
    const client = net.createConnection(socketEndpoint, () => {
      try {
        client.write(`${JSON.stringify(request)}\n`);
      } catch {
        settle(false);
      }
    });
    const settle = (delivered) => {
      if (settled) return;
      settled = true;
      try {
        client.destroy();
      } catch {}
      resolve(delivered);
    };

    client.setTimeout(SOCKET_TIMEOUT_MS, () => settle(false));
    client.on("data", () => settle(true));
    client.on("end", () => settle(true));
    client.on("error", () => settle(false));
    client.on("close", () => settle(false));
  });
}

function setup() {}

async function tui(api) {
  if (
    process.env.HERDR_ENV !== "1" ||
    !process.env.HICODEER_AGENT_SOCKET_PATH ||
    !process.env.HERDR_PANE_ID
  ) {
    return;
  }

  // A host that does not expose these would leave the plugin polling nothing
  // and reporting nothing, which looks identical to "the integration is not
  // installed". Say so once instead of failing silently.
  if (!api?.route || typeof api.state?.session?.status !== "function") {
    console.error(
      "hicodeer deveco hook: TUI plugin API is missing route/state; agent state will not be reported",
    );
    return;
  }

  let disposed = false;
  let chain = Promise.resolve();
  let lastSessionID;
  let lastState;

  function report(method, params) {
    chain = chain
      .then(() => {
        // Queued requests outlive a dispose, so the check belongs here rather
        // than only in `tick`.
        if (disposed) return true;
        return requestOnce(method, params);
      })
      .then((delivered) => {
        // A dropped report has to be retried: the state already moved on, so
        // waiting for the next transition would leave the pane stale forever.
        if (!delivered && !disposed) {
          lastSessionID = undefined;
          lastState = undefined;
        }
      })
      .catch(() => {
        lastSessionID = undefined;
        lastState = undefined;
      });
  }

  // The pane belongs to the root session: a child session opened from it must
  // not take over the pane's identity.
  function rootSessionID(sessionID) {
    let current = sessionID;
    const seen = new Set();
    while (typeof current === "string" && !seen.has(current)) {
      seen.add(current);
      const parent = api.state?.session?.get?.(current)?.parentID;
      if (typeof parent !== "string" || !parent) break;
      current = parent;
    }
    return current;
  }

  // The pane belongs to the root session, but the work may be running in a
  // selected child: the state is read along that chain, and a block anywhere on
  // it blocks the pane.
  function stateOf(selectedSessionID, rootID) {
    const chain =
      selectedSessionID === rootID ? [rootID] : [selectedSessionID, rootID];
    for (const sessionID of chain) {
      const permissions = api.state?.session?.permission?.(sessionID);
      const questions = api.state?.session?.question?.(sessionID);
      if (permissions?.length || questions?.length) {
        return "blocked";
      }
    }
    for (const sessionID of chain) {
      const status = api.state?.session?.status?.(sessionID);
      const kind = typeof status === "string" ? status : status?.type;
      const state =
        typeof kind === "string"
          ? STATE_BY_SESSION_STATUS.get(kind.toLowerCase())
          : undefined;
      if (state) return state;
    }
    return undefined;
  }

  const tick = () => {
    if (disposed) return;
    let sessionID;
    let state;
    try {
      const route = api.route?.current;
      const selected = route?.name === "session" ? route.params?.sessionID : undefined;
      sessionID = selected ? rootSessionID(selected) : undefined;
      if (!sessionID) return;
      state = stateOf(selected, sessionID);
    } catch {
      return;
    }
    if (sessionID !== lastSessionID) {
      lastSessionID = sessionID;
      lastState = undefined;
      report("pane.report_agent_session", { agent_session_id: sessionID });
    }
    if (!state || state === lastState) return;
    lastState = state;
    report("pane.report_agent", { agent_session_id: sessionID, state });
  };

  const poll = setInterval(tick, POLL_INTERVAL_MS);
  tick();

  return () => {
    disposed = true;
    clearInterval(poll);
  };
}

export default {
  id: "herdr.deveco.session-state",
  setup,
  tui,
};

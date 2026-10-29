import { afterEach, beforeEach, expect, mock, test } from "bun:test";

type Request = { method: string; params: Record<string, unknown> };

const requests: Request[] = [];
const activeDisposers: Array<() => void> = [];
let failConnections = false;
let importCounter = 0;

mock.module("node:net", () => ({
  default: {
    createConnection(_path: string, onConnect: () => void) {
      const handlers = new Map<string, () => void>();
      const client = {
        destroyed: false,
        write(input: string) {
          if (client.destroyed) return;
          if (failConnections) {
            handlers.get("error")?.();
            return;
          }
          requests.push(JSON.parse(input.trim()) as Request);
          handlers.get("data")?.();
        },
        setTimeout(_milliseconds: number, handler: () => void) {
          handlers.set("timeout", handler);
          return client;
        },
        on(event: string, handler: () => void) {
          handlers.set(event, handler);
          return client;
        },
        destroy() {
          client.destroyed = true;
          handlers.get("close")?.();
        },
      };
      // The plugin writes from inside the connect callback and touches `client`
      // there, so the callback has to run after the assignment returns.
      queueMicrotask(onConnect);
      return client;
    },
  },
}));

beforeEach(() => {
  requests.length = 0;
  failConnections = false;
  process.env.HICODEER_AGENT_ENV = "1";
  process.env.HICODEER_AGENT_PANE_ID = "w1:p1";
  process.env.HICODEER_AGENT_SOCKET_PATH = "/tmp/agent.sock";
});

afterEach(() => {
  for (const dispose of activeDisposers.splice(0)) {
    dispose();
  }
});

async function loadPlugin() {
  importCounter += 1;
  const module = await import(`./hicodeer-tui-session.js?test=${importCounter}`);
  return module.default;
}

function fakeApi() {
  const sessions = new Map<string, { id: string; parentID?: string }>();
  const statuses = new Map<string, { type: string }>();
  const permissions: Array<{ id: string; sessionID: string }> = [];
  const questions: Array<{ id: string; sessionID: string }> = [];
  let current: { name: string; params?: { sessionID: string } } = { name: "home" };
  let dispose: (() => void) | undefined;
  activeDisposers.push(() => dispose?.());

  return {
    statuses,
    permissions,
    questions,
    api: {
      route: {
        get current() {
          return current;
        },
      },
      state: {
        session: {
          get(sessionID: string) {
            return sessions.get(sessionID);
          },
          status(sessionID: string) {
            return statuses.get(sessionID);
          },
          permission(sessionID: string) {
            return permissions.filter((entry) => entry.sessionID === sessionID);
          },
          question(sessionID: string) {
            return questions.filter((entry) => entry.sessionID === sessionID);
          },
        },
      },
      lifecycle: {
        onDispose(handler: () => void) {
          dispose = handler;
          return () => {};
        },
      },
    },
    addSession(session: { id: string; parentID?: string }) {
      sessions.set(session.id, session);
    },
    select(sessionID: string) {
      current = { name: "session", params: { sessionID } };
    },
    home() {
      current = { name: "home" };
    },
    dispose() {
      dispose?.();
    },
  };
}

async function waitFor(predicate: () => boolean, timeoutMs = 3000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  throw new Error("timed out waiting for the condition");
}

function states(): unknown[] {
  return requests
    .filter((request) => request.method === "pane.report_agent")
    .map((request) => request.params.state);
}

test("stays quiet while no session is selected", async () => {
  const harness = fakeApi();
  const plugin = await loadPlugin();
  const dispose = await plugin.tui(harness.api);
  activeDisposers.push(dispose);

  await new Promise((resolve) => setTimeout(resolve, 400));
  expect(requests).toHaveLength(0);
});

test("reports the selected session and its state changes", async () => {
  const harness = fakeApi();
  harness.addSession({ id: "ses_root" });
  harness.statuses.set("ses_root", { type: "busy" });

  const plugin = await loadPlugin();
  const dispose = await plugin.tui(harness.api);
  activeDisposers.push(dispose);

  harness.select("ses_root");
  await waitFor(() => states().includes("working"));
  expect(
    requests.find((request) => request.method === "pane.report_agent_session")?.params
      .agent_session_id,
  ).toBe("ses_root");

  harness.statuses.set("ses_root", { type: "idle" });
  await waitFor(() => states().includes("idle"));
});

test("reports a child session's activity under the root session", async () => {
  const harness = fakeApi();
  harness.addSession({ id: "ses_root" });
  harness.addSession({ id: "ses_child", parentID: "ses_root" });
  harness.statuses.set("ses_child", { type: "busy" });

  const plugin = await loadPlugin();
  const dispose = await plugin.tui(harness.api);
  activeDisposers.push(dispose);

  harness.select("ses_child");
  await waitFor(() => states().includes("working"));
  const reported = requests.find((request) => request.method === "pane.report_agent_session");
  expect(reported?.params.agent_session_id).toBe("ses_root");
  expect(
    requests
      .filter((request) => request.method === "pane.report_agent")
      .every((request) => request.params.agent_session_id === "ses_root"),
  ).toBe(true);
});

test("maps streaming status to working like the server plugin does", async () => {
  const harness = fakeApi();
  harness.addSession({ id: "ses_root" });
  harness.statuses.set("ses_root", { type: "streaming" });

  const plugin = await loadPlugin();
  const dispose = await plugin.tui(harness.api);
  activeDisposers.push(dispose);

  harness.select("ses_root");
  await waitFor(() => states().includes("working"));
});

test("reports blocked while a permission waits for an answer", async () => {
  const harness = fakeApi();
  harness.addSession({ id: "ses_root" });
  harness.statuses.set("ses_root", { type: "busy" });

  const plugin = await loadPlugin();
  const dispose = await plugin.tui(harness.api);
  activeDisposers.push(dispose);

  harness.select("ses_root");
  await waitFor(() => states().includes("working"));

  harness.permissions.push({ id: "per_1", sessionID: "ses_root" });
  await waitFor(() => states().includes("blocked"));

  harness.permissions.length = 0;
  await waitFor(() => states().includes("working"));
});

test("stops reporting once disposed", async () => {
  const harness = fakeApi();
  harness.addSession({ id: "ses_root" });
  harness.statuses.set("ses_root", { type: "busy" });

  const plugin = await loadPlugin();
  const dispose = await plugin.tui(harness.api);

  harness.select("ses_root");
  await waitFor(() => states().includes("working"));
  dispose();
  const seen = requests.length;

  harness.statuses.set("ses_root", { type: "idle" });
  await new Promise((resolve) => setTimeout(resolve, 500));
  expect(requests.length).toBe(seen);
});

test("retries a report that could not reach the daemon", async () => {
  const harness = fakeApi();
  harness.addSession({ id: "ses_root" });
  harness.statuses.set("ses_root", { type: "busy" });

  const plugin = await loadPlugin();
  const dispose = await plugin.tui(harness.api);
  activeDisposers.push(dispose);

  failConnections = true;
  harness.select("ses_root");
  await new Promise((resolve) => setTimeout(resolve, 300));
  expect(states()).toHaveLength(0);

  failConnections = false;
  await waitFor(() => states().includes("working"));
});

test("does not install when the TUI API is missing", async () => {
  const plugin = await loadPlugin();
  const result = await plugin.tui({});
  expect(result).toBeUndefined();
});

import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/500.css";
import "@fontsource/ibm-plex-sans/600.css";
import "@fontsource/ibm-plex-mono/400.css";
import "@fontsource/ibm-plex-mono/500.css";
import "./icons.css";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { homeDir } from "@tauri-apps/api/path";
import { open, save } from "@tauri-apps/plugin-dialog";
import { emptyArt, type EmptyScene } from "./empty";
import { MAX_LOG_CHARS, latencyLines, statusLabel, type AuthMethod, type Forward, type LatencyReport, type LogLine, type LogPage, type PortNote, type Snapshot, type Tunnel, type TunnelStatus } from "./types";

const summary = document.querySelector<HTMLElement>("#summary")!;
const list = document.querySelector<HTMLElement>("#list")!;
const settingsPath = document.querySelector<HTMLElement>("#settings-path")!;
const settingsError = document.querySelector<HTMLElement>("#settings-error")!;
const dockTitle = document.querySelector<HTMLElement>("#dock-title")!;
const dockMeta = document.querySelector<HTMLElement>("#dock-meta")!;
const dockStatus = document.querySelector<HTMLElement>("#dock-status")!;
const output = document.querySelector<HTMLElement>("#output")!;
const editor = document.querySelector<HTMLDialogElement>("#editor")!;
const editorForm = document.querySelector<HTMLFormElement>("#editor-form")!;
const editorTitle = document.querySelector<HTMLElement>("#editor-title")!;
const editorNote = document.querySelector<HTMLElement>("#editor-note")!;
const formError = document.querySelector<HTMLElement>("#form-error")!;
const forwardsEl = document.querySelector<HTMLElement>("#forwards")!;
const deleteButton = document.querySelector<HTMLButtonElement>("#delete-tunnel")!;
const noteInput = document.querySelector<HTMLInputElement>("#note")!;
const hostInput = document.querySelector<HTMLInputElement>("#host")!;
const portInput = document.querySelector<HTMLInputElement>("#ssh-port")!;
const userInput = document.querySelector<HTMLInputElement>("#user")!;
const passwordInput = document.querySelector<HTMLInputElement>("#password")!;
const useJump = document.querySelector<HTMLInputElement>("#use-jump")!;
const jumpFields = document.querySelector<HTMLElement>("#jump-fields")!;
const jumpHostInput = document.querySelector<HTMLInputElement>("#jump-host")!;
const jumpPortInput = document.querySelector<HTMLInputElement>("#jump-port")!;
const jumpUserInput = document.querySelector<HTMLInputElement>("#jump-user")!;
const jumpPasswordInput = document.querySelector<HTMLInputElement>("#jump-password")!;
const autoReconnect = document.querySelector<HTMLInputElement>("#auto-reconnect")!;
const notifyToggle = document.querySelector<HTMLInputElement>("#notify-toggle")!;

let snapshot: Snapshot = { tunnels: [], selectedId: null, configPath: "", notify: true };
let editingId: string | null = null;
let seenArrival = false;
let shownStatus = "";
let tunnelsLoaded = false;
let machines: TailscaleMachine[] = [];
let tailscaleReport: TailscaleReport | null = null;
let peerKey = "";
let machinesInFlight = false;
let dockLines: StoredLine[] = [];
let logFloor = 0;
let logRequest = 0;

type StoredLine = { seq: number; line: string };

function runningCount(): number {
  return snapshot.tunnels.filter((tunnel) => tunnel.status === "running").length;
}

function connectingCount(): number {
  return snapshot.tunnels.filter((tunnel) => tunnel.status === "connecting").length;
}

function waitingCount(): number {
  return snapshot.tunnels.filter((tunnel) => tunnel.status === "waiting").length;
}

/** Running, connecting, or waiting to reconnect: "stop" is the useful action. */
function busy(status: TunnelStatus): boolean {
  return status === "running" || status === "connecting" || status === "waiting";
}

function render() {
  const count = snapshot.tunnels.length;
  const running = runningCount();
  const connecting = connectingCount();
  const waiting = waitingCount();
  summary.textContent = count === 0
    ? "没有隧道"
    : running > 0
      ? `${running} 条在跑`
      : connecting > 0
        ? "正在连接"
        : waiting > 0
          ? "等待重连"
          : "没有在跑的隧道";
  summary.className = running > 0 || connecting > 0 ? "info live" : "info";
  settingsPath.textContent = snapshot.configPath;
  notifyToggle.checked = snapshot.notify;
  syncDock();

  list.classList.toggle("arrive", !seenArrival);
  seenArrival = true;
  list.replaceChildren();
  if (count === 0) {
    const create = labeledButton("add-line", "新建隧道", "accent");
    create.addEventListener("click", () => openEditor());
    list.append(emptyState("no-tunnels", "还没有隧道", "新建一条，填上主机、用户和本地转发，保存后点启动。", create));
    return;
  }

  for (const tunnel of snapshot.tunnels) {
    const row = document.createElement("div");
    row.className = tunnel.id === snapshot.selectedId ? `row ${tunnel.status} selected` : `row ${tunnel.status}`;
    row.addEventListener("click", (event) => {
      if (event.target instanceof Element && event.target.closest("button:not(.row-main)")) return;
      void select(tunnel.id);
    });

    const tile = document.createElement("span");
    tile.className = "tile";
    tile.append(icon("route-line"));

    const main = document.createElement("button");
    main.type = "button";
    main.className = "row-main";
    const identity = document.createElement("span");
    identity.className = "identity";
    identity.textContent = tunnel.note || `${tunnel.user}@${tunnel.host}`;
    main.append(identity);
    if (tunnel.note) {
      const sub = document.createElement("span");
      sub.className = "sub";
      sub.textContent = `${tunnel.user}@${tunnel.host}`;
      main.append(sub);
    }
    if (tunnel.jump) {
      const via = document.createElement("span");
      via.className = "via";
      via.title = `经跳板 ${tunnel.jump.user}@${tunnel.jump.host}`;
      via.append(icon("route-line"), document.createTextNode(`经 ${tunnel.jump.host}`));
      main.append(via);
    }
    const peer = peerOf(tunnel.host);
    if (peer) {
      const badge = document.createElement("span");
      badge.className = peer.online ? "peer on" : "peer off";
      badge.textContent = peer.online ? "在线" : "离线";
      main.append(badge);
    }

    const detail = document.createElement("div");
    detail.className = "detail";
    fillForwards(detail, tunnel.forwards);

    const status = document.createElement("span");
    paintStatus(status, tunnel.status, tunnel.connectedAt);

    const actions = document.createElement("div");
    actions.className = "row-actions";
    const action = busy(tunnel.status)
      ? labeledButton("stop-line", "停止", "stop")
      : labeledButton("play-line", "启动", "go");
    action.addEventListener("click", () => {
      void toggle(tunnel);
    });

    const edit = document.createElement("button");
    edit.type = "button";
    edit.className = "icon";
    edit.title = "编辑";
    edit.setAttribute("aria-label", "编辑");
    edit.append(icon("pencil-line"));
    edit.addEventListener("click", () => openEditor(tunnel));
    const probing = latencyState.get(tunnel.id) === "pending";
    const measure = document.createElement("button");
    measure.type = "button";
    measure.className = "icon";
    measure.title = tunnel.jump ? "测速：比较直连和经跳板的延迟" : "测速";
    measure.setAttribute("aria-label", "测速");
    measure.disabled = probing;
    measure.append(icon(probing ? "loader-4-line" : "timer-line"));
    if (probing) measure.firstElementChild?.classList.add("spin");
    measure.addEventListener("click", () => {
      void measureLatency(tunnel);
    });
    actions.append(action, measure, edit);

    row.append(tile, main, detail, status, actions);
    if (tunnel.message) {
      const message = document.createElement("span");
      message.className = "message";
      message.append(icon("error-warning-line"), document.createTextNode(tunnel.message));
      row.append(message);
    }
    const latency = latencyState.get(tunnel.id);
    if (latency) {
      const line = document.createElement("span");
      if (latency === "pending") {
        line.className = "latency";
        line.append(icon("timer-line"), document.createTextNode("正在测速…"));
      } else if (typeof latency === "string") {
        line.className = "latency bad";
        line.append(icon("error-warning-line"), document.createTextNode(latency));
      } else {
        line.className = "latency";
        line.append(icon("timer-line"), document.createTextNode(latencyLines(latency, Boolean(tunnel.jump)).join(" · ")));
      }
      row.append(line);
    }
    list.append(row);
  }
}

const latencyState = new Map<string, LatencyReport | "pending" | string>();

async function measureLatency(tunnel: Tunnel) {
  if (latencyState.get(tunnel.id) === "pending") return;
  latencyState.set(tunnel.id, "pending");
  render();
  try {
    latencyState.set(tunnel.id, await invoke<LatencyReport>("measure_latency", { id: tunnel.id }));
  } catch (error) {
    latencyState.set(tunnel.id, errorText(error));
  }
  render();
}

function icon(name: string): HTMLElement {
  const el = document.createElement("i");
  el.className = `ri-${name}`;
  el.setAttribute("aria-hidden", "true");
  return el;
}

function labeledButton(iconName: string, label: string, className = ""): HTMLButtonElement {
  const button = document.createElement("button");
  button.type = "button";
  if (className) button.className = className;
  const text = document.createElement("span");
  text.textContent = label;
  button.append(icon(iconName), text);
  return button;
}

function emptyState(scene: EmptyScene, title: string, text: string, action?: HTMLElement): HTMLElement {
  const box = document.createElement("div");
  box.className = "empty";
  const art = document.createElement("div");
  art.className = "empty-art";
  art.innerHTML = emptyArt(scene);
  const heading = document.createElement("p");
  heading.className = "empty-title";
  heading.textContent = title;
  const line = document.createElement("p");
  line.className = "empty-text";
  line.textContent = text;
  box.append(art, heading, line);
  if (action) box.append(action);
  return box;
}

async function refresh() {
  snapshot = await invoke<Snapshot>("list_tunnels");
  tunnelsLoaded = true;
  render();
}

async function select(id: string) {
  snapshot = await invoke<Snapshot>("select_tunnel", { id });
  render();
}

const pendingTunnels = new Set<string>();

async function toggle(tunnel: Tunnel) {
  if (pendingTunnels.has(tunnel.id)) return;
  pendingTunnels.add(tunnel.id);
  const command = busy(tunnel.status) ? "stop_tunnel" : "start_tunnel";
  try {
    snapshot = await invoke<Snapshot>(command, { id: tunnel.id });
    render();
  } catch (error) {
    await refresh();
    summary.textContent = errorText(error);
    summary.className = "info bad";
  } finally {
    pendingTunnels.delete(tunnel.id);
  }
}

function errorText(error: unknown): string {
  if (typeof error === "string") return error;
  return "没有完成这次操作。";
}

function blankForward(): Forward {
  return { localPort: 0, remoteHost: "127.0.0.1", remotePort: 0 };
}

function addForwardRow(forward: Forward = blankForward()) {
  const item = document.createElement("div");
  item.className = "forward-item";
  const hint = document.createElement("p");
  hint.className = "port-hint";
  hint.hidden = true;
  const row = document.createElement("div");
  row.className = "forward-row";
  const local = field("本地端口", String(forward.localPort || ""), "number");
  local.addEventListener("input", schedulePortCheck);
  const remoteHost = field("远端地址", forward.remoteHost, "text");
  const remote = field("远端端口", String(forward.remotePort || ""), "number");
  const arrow = icon("arrow-right-line");
  arrow.className += " forward-arrow";
  const remove = document.createElement("button");
  remove.type = "button";
  remove.className = "icon";
  remove.append(icon("close-line"));
  remove.setAttribute("aria-label", "去掉这一行");
  remove.addEventListener("click", () => {
    if (forwardsEl.querySelectorAll(".forward-row").length === 1) return;
    item.remove();
    syncRemoveButtons();
    schedulePortCheck();
  });
  row.append(local, arrow, remoteHost, remote, remove);
  item.append(row, hint);
  forwardsEl.append(item);
  syncRemoveButtons();
}

let portCheckTimer = 0;
let portCheckRequest = 0;

function schedulePortCheck() {
  window.clearTimeout(portCheckTimer);
  portCheckTimer = window.setTimeout(() => {
    void checkPorts();
  }, 350);
}

function setHint(item: Element, message: string) {
  const hint = item.querySelector<HTMLElement>(".port-hint")!;
  hint.hidden = message === "";
  if (message) hint.replaceChildren(icon("error-warning-line"), document.createTextNode(message));
  else hint.replaceChildren();
}

/** Warns about repeated local ports, other tunnels on the same port, and programs already listening. */
async function checkPorts() {
  const request = ++portCheckRequest;
  const items = [...forwardsEl.querySelectorAll<HTMLElement>(".forward-item")];
  const ports = items.map((item) => Number(item.querySelector("input")!.value));
  const seen = new Set<number>();
  const repeated = ports.map((port) => {
    if (!Number.isInteger(port) || port < 1 || port > 65535) return false;
    const again = seen.has(port);
    seen.add(port);
    return again;
  });
  let notes: PortNote[] = [];
  try {
    notes = await invoke<PortNote[]>("check_ports", { id: editingId, ports: [...seen] });
  } catch {
    notes = [];
  }
  if (request !== portCheckRequest) return;
  const byPort = new Map(notes.map((note) => [note.port, note.message]));
  items.forEach((item, index) => {
    if (repeated[index]) setHint(item, "这个端口上面已经填过了，同一条隧道里不能重复。");
    else setHint(item, byPort.get(ports[index]) ?? "");
  });
}

function syncRemoveButtons() {
  const buttons = forwardsEl.querySelectorAll<HTMLButtonElement>("button.icon");
  const only = buttons.length === 1;
  for (const button of buttons) button.disabled = only;
}

function field(placeholder: string, value: string, type: string): HTMLInputElement {
  const input = document.createElement("input");
  input.placeholder = placeholder;
  input.value = value === "0" ? "" : value;
  input.type = type;
  if (type === "number") {
    input.min = "1";
    input.max = "65535";
    input.inputMode = "numeric";
  }
  input.autocomplete = "off";
  input.autocapitalize = "off";
  input.spellcheck = false;
  input.setAttribute("autocorrect", "off");
  input.setAttribute("aria-label", placeholder);
  return input;
}

function readForwards(): Forward[] {
  return [...forwardsEl.querySelectorAll<HTMLElement>(".forward-row")].map((row) => {
    const inputs = row.querySelectorAll("input");
    return {
      localPort: Number(inputs[0].value),
      remoteHost: inputs[1].value,
      remotePort: Number(inputs[2].value),
    };
  });
}

function showError(message: string) {
  formError.hidden = false;
  formError.textContent = message;
}

function openEditor(tunnel?: Tunnel) {
  editingId = tunnel?.id ?? null;
  editorTitle.textContent = tunnel ? "编辑隧道" : "新建隧道";
  editorNote.textContent = tunnel && busy(tunnel.status)
    ? "下次启动才生效。"
    : "";
  noteInput.value = tunnel?.note ?? "";
  hostInput.value = tunnel?.host ?? "";
  portInput.value = String(tunnel?.port ?? 22);
  userInput.value = tunnel?.user ?? "";
  passwordInput.value = tunnel?.password ?? "";
  passwordInput.type = "password";
  paintPasswordToggle(false);
  mainAuth.keyInput.value = tunnel?.keyFile ?? "";
  mainAuth.set(tunnel?.auth ?? "password");
  autoReconnect.checked = tunnel?.autoReconnect ?? false;
  useJump.checked = Boolean(tunnel?.jump);
  jumpHostInput.value = tunnel?.jump?.host ?? "";
  jumpPortInput.value = String(tunnel?.jump?.port ?? 22);
  jumpUserInput.value = tunnel?.jump?.user ?? "";
  jumpPasswordInput.value = tunnel?.jump?.password ?? "";
  jumpAuth.keyInput.value = tunnel?.jump?.keyFile ?? "";
  jumpAuth.set(tunnel?.jump?.auth ?? "password");
  syncJumpFields();
  deleteButton.hidden = !tunnel;
  document.querySelector<HTMLButtonElement>("#copy-tunnel-edit")!.hidden = !tunnel;
  document.querySelector<HTMLButtonElement>("#export-tunnel-edit")!.hidden = !tunnel;
  document.querySelector<HTMLElement>("#host-picks")!.hidden = true;
  formError.hidden = true;
  forwardsEl.replaceChildren();
  const forwards = tunnel?.forwards.length ? tunnel.forwards : [blankForward()];
  for (const forward of forwards) addForwardRow(forward);
  editor.showModal();
  noteInput.focus();
  void checkPorts();
}

type AuthControl = {
  keyInput: HTMLInputElement;
  method: () => AuthMethod;
  set: (next: AuthMethod) => void;
};

/** Wires the 密码/私钥 switch of one login (the tunnel's, or the jump's when `prefix` is "jump-"). */
function authControl(prefix: "" | "jump-", passwordText: string, passphraseText: string): AuthControl {
  const byId = <T extends HTMLElement>(id: string) => document.querySelector<T>(`#${prefix}${id}`)!;
  const passwordButton = byId<HTMLButtonElement>("auth-password");
  const keyButton = byId<HTMLButtonElement>("auth-key");
  const keyField = byId<HTMLElement>("key-field");
  const keyInput = byId<HTMLInputElement>("key-file");
  const secretLabel = byId<HTMLElement>("password-label");
  const secretInput = byId<HTMLInputElement>("password");
  let current: AuthMethod = "password";
  const set = (next: AuthMethod) => {
    current = next;
    const key = next === "key";
    passwordButton.classList.toggle("current", !key);
    keyButton.classList.toggle("current", key);
    passwordButton.setAttribute("aria-pressed", String(!key));
    keyButton.setAttribute("aria-pressed", String(key));
    keyField.hidden = !key;
    secretLabel.textContent = key ? passphraseText : passwordText;
    secretInput.placeholder = key ? "没有口令就留空" : "";
  };
  passwordButton.addEventListener("click", () => set("password"));
  keyButton.addEventListener("click", () => {
    set("key");
    if (!keyInput.value.trim()) keyInput.focus();
  });
  byId<HTMLButtonElement>("pick-key").addEventListener("click", () => {
    void pickKeyFile(keyInput);
  });
  return { keyInput, method: () => current, set };
}

async function pickKeyFile(target: HTMLInputElement) {
  let start: string | undefined;
  const typed = target.value.trim();
  if (typed.startsWith("/")) {
    start = typed;
  } else {
    try {
      start = `${(await homeDir()).replace(/\/$/, "")}/.ssh`;
    } catch {
      start = undefined;
    }
  }
  try {
    const picked = await open({ title: "选择私钥文件", multiple: false, directory: false, defaultPath: start });
    if (typeof picked === "string") target.value = picked;
  } catch (error) {
    formError.hidden = false;
    formError.textContent = errorText(error);
  }
}

const mainAuth = authControl("", "密码", "私钥口令");
const jumpAuth = authControl("jump-", "跳板密码", "跳板口令");

function syncJumpFields() {
  jumpFields.hidden = !useJump.checked;
}

useJump.addEventListener("change", () => {
  syncJumpFields();
  if (useJump.checked) jumpHostInput.focus();
});

function readJump() {
  if (!useJump.checked) return null;
  return {
    host: jumpHostInput.value,
    port: Number(jumpPortInput.value),
    user: jumpUserInput.value,
    password: jumpPasswordInput.value,
    auth: jumpAuth.method(),
    keyFile: jumpAuth.keyInput.value,
  };
}

function syncDock() {
  const tunnel = snapshot.tunnels.find((item) => item.id === snapshot.selectedId);
  const status = tunnel?.status ?? "";
  const previousId = output.dataset.tunnelId ?? "";
  const nextId = snapshot.selectedId ?? "";
  const changed = previousId !== nextId || status !== shownStatus;
  output.dataset.tunnelId = nextId;
  shownStatus = status;
  const dock = output.closest(".dock");
  if (!tunnel) {
    dock?.classList.add("idle");
    dockTitle.textContent = "";
    dockMeta.replaceChildren();
    dockStatus.textContent = "";
    dockStatus.className = "status stopped";
    delete dockStatus.dataset.connectedAt;
    delete dockStatus.dataset.statusBase;
    dockLines = [];
    logFloor = 0;
    logRequest += 1;
    output.textContent = "";
    return;
  }
  dock?.classList.remove("idle");
  dockTitle.textContent = tunnel.note || `${tunnel.user}@${tunnel.host}`;
  dockTitle.title = tunnel.note ? `${tunnel.user}@${tunnel.host}` : "";
  fillForwards(dockMeta, tunnel.forwards, true);
  paintStatus(dockStatus, tunnel.status, tunnel.connectedAt);
  if (changed) {
    if (previousId !== nextId) {
      dockLines = [];
      logFloor = 0;
      output.textContent = "";
    }
    const request = ++logRequest;
    void invoke<LogPage>("tunnel_log", { id: nextId }).then((page) => {
      if (request !== logRequest || output.dataset.tunnelId !== nextId) return;
      setLog(page);
    });
  }
}

function peerOf(host: string): TailscaleMachine | undefined {
  const trimmed = host.trim();
  const key = trimmed.toLowerCase();
  return machines.find((machine) => {
    if (machine.ipv4.length > 0 && machine.ipv4 === trimmed) return true;
    if (machine.hostName.toLowerCase() === key) return true;
    return machine.dnsName.length > 0 && machine.dnsName.toLowerCase() === key;
  });
}

function fillForwards(target: HTMLElement, forwards: Forward[], compact = false) {
  target.replaceChildren();
  if (compact) {
    for (const forward of forwards) {
      const line = document.createElement("span");
      line.className = "forward-text";
      line.textContent = `${forward.localPort} → ${forward.remoteHost}:${forward.remotePort}`;
      target.append(line);
    }
    return;
  }
  for (const forward of forwards) {
    const chip = document.createElement("span");
    chip.className = "forward";
    const port = document.createElement("button");
    port.type = "button";
    port.className = "port";
    port.title = `用浏览器打开 localhost:${forward.localPort}`;
    port.append(document.createTextNode(String(forward.localPort)), icon("external-link-line"));
    port.setAttribute("aria-label", `用浏览器打开 localhost:${forward.localPort}`);
    port.addEventListener("click", () => {
      void invoke("open_local", { port: forward.localPort }).catch((error: unknown) => {
        summary.textContent = errorText(error);
        summary.className = "info bad";
      });
    });
    const remote = document.createElement("span");
    remote.className = "remote";
    remote.textContent = `${forward.remoteHost}:${forward.remotePort}`;
    chip.append(port, icon("arrow-right-line"), remote);
    target.append(chip);
  }
}

function paintStatus(el: HTMLElement, status: TunnelStatus, connectedAt: number | null) {
  el.className = `status ${status}`;
  const base = statusLabel(status);
  el.dataset.statusBase = base;
  if (status === "running" && connectedAt) {
    el.dataset.connectedAt = String(connectedAt);
    el.textContent = `${base} ${formatDuration(Date.now() - connectedAt)}`;
  } else {
    delete el.dataset.connectedAt;
    el.textContent = base;
  }
}

function formatDuration(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  const pad = (value: number) => String(value).padStart(2, "0");
  if (hours > 0) return `${hours}:${pad(minutes)}:${pad(seconds)}`;
  return `${minutes}:${pad(seconds)}`;
}

function byteLength(text: string): number {
  return new TextEncoder().encode(text).length;
}

function logChars(lines: StoredLine[]): number {
  if (lines.length === 0) return 0;
  return lines.reduce((sum, item) => sum + byteLength(item.line), 0) + lines.length - 1;
}

function trimDockLog() {
  while (dockLines.length > 1 && logChars(dockLines) > MAX_LOG_CHARS) dockLines.shift();
}

function paintLog(stick: boolean) {
  output.textContent = dockLines.map((item) => item.line).join("\n");
  if (stick) output.scrollTop = output.scrollHeight;
}

function setLog(page: LogPage) {
  logFloor = page.floor;
  const maxSeq = page.lines.reduce((max, item) => Math.max(max, item.seq), -1);
  const seen = new Set(page.lines.map((item) => item.seq));
  const newer = dockLines.filter((item) => item.seq >= page.floor && item.seq > maxSeq && !seen.has(item.seq));
  dockLines = [...page.lines, ...newer].sort((left, right) => left.seq - right.seq);
  trimDockLog();
  paintLog(true);
}

function pushDockLine(entry: StoredLine, stick: boolean) {
  if (entry.seq < logFloor) return;
  if (dockLines.some((item) => item.seq === entry.seq)) return;
  if (dockLines.length > 0 && entry.seq < dockLines[0].seq) return;
  dockLines.push(entry);
  dockLines.sort((left, right) => left.seq - right.seq);
  trimDockLog();
  paintLog(stick);
}

function copyText(tunnel: Tunnel): string {
  const head = tunnel.port === 22
    ? `${tunnel.user}@${tunnel.host}`
    : `${tunnel.user}@${tunnel.host}:${tunnel.port}`;
  const forwards = tunnel.forwards.map(
    (forward) => `${forward.localPort} → ${forward.remoteHost}:${forward.remotePort}`,
  );
  return [head, ...forwards].join("\n");
}

function selectedTunnel(): Tunnel | undefined {
  return snapshot.tunnels.find((item) => item.id === snapshot.selectedId);
}

function tunnelById(id: string | null): Tunnel | undefined {
  if (!id) return undefined;
  return snapshot.tunnels.find((item) => item.id === id);
}

function flashLabel(button: HTMLButtonElement, text: string) {
  const label = button.querySelector("span") ?? button;
  const original = button.dataset.label ?? label.textContent ?? "";
  button.dataset.label = original;
  label.textContent = text;
  button.classList.add("done");
  window.setTimeout(() => {
    label.textContent = original;
    button.classList.remove("done");
  }, 1200);
}

async function copyTunnel(tunnel: Tunnel, button: HTMLButtonElement) {
  try {
    await navigator.clipboard.writeText(copyText(tunnel));
    flashLabel(button, "已复制");
  } catch {
    summary.textContent = "没有复制。";
    summary.className = "info bad";
  }
}

async function exportTunnel(tunnel: Tunnel, button: HTMLButtonElement) {
  try {
    const picked = await save({
      title: "导出隧道",
      defaultPath: `${tunnel.host}.json`,
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    if (typeof picked !== "string") return;
    await invoke("export_tunnel", { id: tunnel.id, path: picked });
    flashLabel(button, "已导出");
  } catch (error) {
    summary.textContent = errorText(error);
    summary.className = "info bad";
  }
}

document.querySelector<HTMLButtonElement>("#pick-config")!.addEventListener("click", () => {
  settingsError.hidden = true;
  void open({
    title: "配置文件",
    multiple: false,
    directory: false,
    defaultPath: snapshot.configPath,
    filters: [{ name: "JSON", extensions: ["json"] }],
  }).then(async (picked) => {
    if (typeof picked !== "string") return;
    try {
      snapshot = await invoke<Snapshot>("set_config_path", { path: picked });
      render();
    } catch (error) {
      settingsError.hidden = false;
      settingsError.textContent = errorText(error);
    }
  }).catch((error: unknown) => {
    settingsError.hidden = false;
    settingsError.textContent = errorText(error);
  });
});

notifyToggle.addEventListener("change", () => {
  const on = notifyToggle.checked;
  settingsError.hidden = true;
  void invoke<Snapshot>("set_notify", { on })
    .then((next) => {
      snapshot = next;
      render();
    })
    .catch((error: unknown) => {
      notifyToggle.checked = snapshot.notify;
      settingsError.hidden = false;
      settingsError.textContent = errorText(error);
    });
});

document.querySelector<HTMLButtonElement>("#create")!.addEventListener("click", () => openEditor());

type AppView = "tunnels" | "tailscale" | "settings";

type TailscaleMachine = {
  hostName: string;
  dnsName: string;
  ipv4: string;
  online: boolean;
  here: boolean;
  path: string;
};

type TailscaleReport = {
  installed: boolean;
  stateLabel: string;
  machines: TailscaleMachine[];
  message: string;
};

const viewTitles: Record<AppView, string> = { tunnels: "隧道", tailscale: "Tailscale", settings: "设置" };

function showView(view: AppView) {
  document.body.dataset.view = view;
  document.querySelector<HTMLElement>("#page-title")!.textContent = viewTitles[view];
  document.querySelector<HTMLElement>("#view-tunnels")!.hidden = view !== "tunnels";
  document.querySelector<HTMLElement>("#view-tailscale")!.hidden = view !== "tailscale";
  document.querySelector<HTMLElement>("#view-settings")!.hidden = view !== "settings";
  document.querySelector("#nav-tunnels")!.classList.toggle("current", view === "tunnels");
  document.querySelector("#nav-tailscale")!.classList.toggle("current", view === "tailscale");
  document.querySelector("#nav-settings")!.classList.toggle("current", view === "settings");
  if (view === "tailscale") void refreshMachines(true);
}

function paintTailscale(report: TailscaleReport) {
  const body = document.querySelector<HTMLElement>("#tailscale-body")!;
  body.replaceChildren();
  if (!report.installed || report.message) {
    body.append(emptyState("no-tailscale", "读不到 Tailscale", report.message || "这台 Mac 上没有可用的 Tailscale。"));
    return;
  }
  const card = document.createElement("div");
  card.className = "card machine-list";
  for (const machine of report.machines) {
    const row = document.createElement("div");
    row.className = machine.online ? "machine live" : "machine";
    const tile = document.createElement("span");
    tile.className = "tile";
    tile.append(icon(machine.here ? "macbook-line" : "server-line"));
    const name = document.createElement("span");
    name.className = "machine-name";
    const host = document.createElement("span");
    host.className = "identity";
    host.textContent = machine.hostName;
    name.append(host);
    if (machine.here) {
      const here = document.createElement("span");
      here.className = "here";
      here.textContent = "本机";
      name.append(here);
    }
    const address = document.createElement("span");
    address.className = "machine-address";
    const ip = document.createElement("span");
    ip.className = "ip";
    ip.textContent = machine.ipv4;
    const copy = document.createElement("button");
    copy.type = "button";
    copy.className = "icon copy";
    copy.title = "复制地址";
    copy.setAttribute("aria-label", "复制");
    copy.append(icon("file-copy-line"));
    copy.disabled = machine.ipv4.length === 0;
    copy.addEventListener("click", () => {
      void navigator.clipboard.writeText(machine.ipv4).then(() => {
        copy.replaceChildren(icon("check-line"));
        copy.classList.add("done");
        copy.setAttribute("aria-label", "已复制");
        window.setTimeout(() => {
          copy.replaceChildren(icon("file-copy-line"));
          copy.classList.remove("done");
          copy.setAttribute("aria-label", "复制");
        }, 1200);
      });
    });
    address.append(ip, copy);
    const state = document.createElement("span");
    state.className = "machine-state";
    if (machine.path) {
      const path = document.createElement("span");
      path.className = machine.path.startsWith("DERP") ? "path derp" : "path direct";
      path.textContent = machine.path;
      state.append(path);
    }
    const presence = document.createElement("span");
    presence.className = machine.online ? "status running" : "status stopped";
    presence.textContent = machine.online ? "Online" : "Offline";
    state.append(presence);
    row.append(tile, name, address, state);
    card.append(row);
  }
  if (report.machines.length > 0) body.append(card);
  const others = report.machines.filter((machine) => !machine.here);
  if (others.length === 0) {
    const again = labeledButton("refresh-line", "再看一次", "accent");
    again.addEventListener("click", () => {
      void refreshMachines(true);
    });
    body.append(emptyState(
      "lonely-tailnet",
      "网里还没有其他机器",
      "在别的设备上登录同一个 Tailscale 账号，它就会出现在这里。",
      again,
    ));
  }
}

function pendingNote(): HTMLElement {
  const line = document.createElement("p");
  line.className = "notice pending";
  line.append(icon("loader-4-line"), document.createTextNode("正在读取"));
  return line;
}

function fillHostPicks() {
  const box = document.querySelector<HTMLElement>("#host-picks")!;
  box.replaceChildren();
  const note = (text: string) => {
    const line = document.createElement("p");
    line.className = "notice";
    line.textContent = text;
    box.append(line);
  };
  if (!tailscaleReport) {
    box.append(pendingNote());
    return;
  }
  if (!tailscaleReport.installed || tailscaleReport.message) {
    note(tailscaleReport.message || "读不到 Tailscale。");
    return;
  }
  const others = tailscaleReport.machines.filter((machine) => !machine.here && machine.ipv4.length > 0);
  if (others.length === 0) {
    note("网里还没有其他机器。");
    return;
  }
  for (const machine of others) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "host-pick";
    const name = document.createElement("span");
    name.className = "identity";
    name.textContent = machine.hostName;
    const ip = document.createElement("span");
    ip.className = "ip";
    ip.textContent = machine.ipv4;
    const presence = document.createElement("span");
    presence.className = machine.online ? "peer on" : "peer off";
    presence.textContent = machine.path
      ? `${machine.online ? "在线" : "离线"} · ${machine.path}`
      : machine.online ? "在线" : "离线";
    button.append(icon("server-line"), name, ip, presence);
    button.addEventListener("click", () => {
      hostInput.value = machine.ipv4;
      box.hidden = true;
      hostInput.focus();
    });
    box.append(button);
  }
}

async function refreshMachines(manual: boolean) {
  if (machinesInFlight) return;
  machinesInFlight = true;
  const body = document.querySelector<HTMLElement>("#tailscale-body")!;
  if (manual && document.body.dataset.view === "tailscale") {
    body.replaceChildren(pendingNote());
  }
  try {
    const report = await invoke<TailscaleReport>("tailscale_status");
    tailscaleReport = report;
    if (report.installed && !report.message) {
      machines = report.machines;
      const nextKey = machines
        .map((machine) => `${machine.ipv4}\t${machine.online}\t${machine.hostName}\t${machine.dnsName}`)
        .join("\n");
      if (tunnelsLoaded && nextKey !== peerKey) render();
      peerKey = nextKey;
    }
    if (document.body.dataset.view === "tailscale") paintTailscale(report);
    const picks = document.querySelector<HTMLElement>("#host-picks")!;
    if (!picks.hidden) fillHostPicks();
  } catch (error) {
    if (document.body.dataset.view === "tailscale") {
      body.replaceChildren(emptyState("no-tailscale", "读不到 Tailscale", errorText(error)));
    }
    const picks = document.querySelector<HTMLElement>("#host-picks")!;
    if (!picks.hidden && !tailscaleReport) {
      picks.replaceChildren();
      const line = document.createElement("p");
      line.className = "notice";
      line.textContent = errorText(error);
      picks.append(line);
    }
  } finally {
    machinesInFlight = false;
  }
}

document.querySelector("#nav-tunnels")!.addEventListener("click", () => showView("tunnels"));
document.querySelector("#nav-tailscale")!.addEventListener("click", () => showView("tailscale"));
document.querySelector("#nav-settings")!.addEventListener("click", () => showView("settings"));
document.querySelector("#refresh-tailscale")!.addEventListener("click", () => {
  void refreshMachines(true);
});
document.querySelector<HTMLButtonElement>("#pick-host")!.addEventListener("click", () => {
  const picks = document.querySelector<HTMLElement>("#host-picks")!;
  if (!picks.hidden) {
    picks.hidden = true;
    return;
  }
  picks.hidden = false;
  fillHostPicks();
  void refreshMachines(false);
});
editor.addEventListener("close", () => {
  document.querySelector<HTMLElement>("#host-picks")!.hidden = true;
});
document.querySelector<HTMLButtonElement>("#copy-tunnel")!.addEventListener("click", () => {
  const tunnel = selectedTunnel();
  if (!tunnel) return;
  void copyTunnel(tunnel, document.querySelector<HTMLButtonElement>("#copy-tunnel")!);
});
document.querySelector<HTMLButtonElement>("#export-tunnel")!.addEventListener("click", () => {
  const tunnel = selectedTunnel();
  if (!tunnel) return;
  void exportTunnel(tunnel, document.querySelector<HTMLButtonElement>("#export-tunnel")!);
});
document.querySelector<HTMLButtonElement>("#copy-tunnel-edit")!.addEventListener("click", () => {
  const tunnel = tunnelById(editingId);
  if (!tunnel) return;
  void copyTunnel(tunnel, document.querySelector<HTMLButtonElement>("#copy-tunnel-edit")!);
});
document.querySelector<HTMLButtonElement>("#export-tunnel-edit")!.addEventListener("click", () => {
  const tunnel = tunnelById(editingId);
  if (!tunnel) return;
  void exportTunnel(tunnel, document.querySelector<HTMLButtonElement>("#export-tunnel-edit")!);
});
document.querySelector<HTMLButtonElement>("#add-forward")!.addEventListener("click", () => addForwardRow());
document.querySelector<HTMLButtonElement>("#cancel-edit")!.addEventListener("click", () => editor.close());
document.querySelector<HTMLButtonElement>("#close-edit")!.addEventListener("click", () => editor.close());
document.querySelector<HTMLButtonElement>("#toggle-password")!.addEventListener("click", () => {
  const show = passwordInput.type === "password";
  passwordInput.type = show ? "text" : "password";
  paintPasswordToggle(show);
});

function paintPasswordToggle(shown: boolean) {
  const button = document.querySelector<HTMLButtonElement>("#toggle-password")!;
  button.replaceChildren(icon(shown ? "eye-off-line" : "eye-line"));
  button.setAttribute("aria-label", shown ? "隐藏密码" : "显示密码");
  button.title = shown ? "隐藏密码" : "显示密码";
}

deleteButton.addEventListener("click", () => {
  if (!editingId) return;
  if (!window.confirm("删除这条隧道？正在运行的连接会停掉。")) return;
  void invoke<Snapshot>("delete_tunnel", { id: editingId })
    .then((next) => {
      snapshot = next;
      editor.close();
      render();
    })
    .catch((error: unknown) => showError(errorText(error)));
});

editorForm.addEventListener("submit", (event) => {
  event.preventDefault();
  formError.hidden = true;
  const draft = {
    id: editingId,
    note: noteInput.value,
    host: hostInput.value,
    port: Number(portInput.value),
    user: userInput.value,
    password: passwordInput.value,
    auth: mainAuth.method(),
    keyFile: mainAuth.keyInput.value,
    jump: readJump(),
    forwards: readForwards(),
    autoReconnect: autoReconnect.checked,
  };
  void invoke<Snapshot>("save_tunnel", { draft })
    .then((next) => {
      snapshot = next;
      editor.close();
      render();
    })
    .catch((error: unknown) => showError(errorText(error)));
});

void listen<Snapshot>("tunnels-changed", (event) => {
  snapshot = event.payload;
  render();
});

void listen<LogLine>("tunnel-log", (event) => {
  if (event.payload.id !== snapshot.selectedId) return;
  const nearBottom = output.scrollHeight - output.scrollTop - output.clientHeight < 48;
  pushDockLine({ seq: event.payload.seq, line: event.payload.line }, nearBottom);
});

window.setInterval(() => {
  const now = Date.now();
  for (const el of document.querySelectorAll<HTMLElement>("[data-connected-at]")) {
    const at = Number(el.dataset.connectedAt);
    const base = el.dataset.statusBase ?? "运行中";
    if (!Number.isFinite(at)) continue;
    el.textContent = `${base} ${formatDuration(now - at)}`;
  }
}, 1000);

void refresh().catch((error: unknown) => {
  summary.textContent = errorText(error);
  summary.className = "info bad";
});
void refreshMachines(false);
window.setInterval(() => {
  void refreshMachines(false);
}, 15000);

import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/500.css";
import "@fontsource/ibm-plex-sans/600.css";
import "@fontsource/ibm-plex-mono/400.css";
import "@fontsource/ibm-plex-mono/500.css";
import "./icons.css";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { homeDir } from "@tauri-apps/api/path";
import { confirm, open, save } from "@tauri-apps/plugin-dialog";
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
const syncCard = document.querySelector<HTMLElement>("#sync-card")!;
const syncGlyph = document.querySelector<HTMLElement>("#sync-glyph")!;
const syncState = document.querySelector<HTMLElement>("#sync-state")!;
const syncIntro = document.querySelector<HTMLElement>("#sync-intro")!;
const syncDevices = document.querySelector<HTMLElement>("#sync-devices")!;
const syncReport = document.querySelector<HTMLElement>("#sync-report")!;
const syncError = document.querySelector<HTMLElement>("#sync-error")!;
const syncFolder = document.querySelector<HTMLElement>("#sync-folder")!;
const syncNowButton = document.querySelector<HTMLButtonElement>("#sync-now")!;
const syncUnlockButton = document.querySelector<HTMLButtonElement>("#sync-unlock")!;
const syncEnableButton = document.querySelector<HTMLButtonElement>("#sync-enable")!;
const syncDisableButton = document.querySelector<HTMLButtonElement>("#sync-disable")!;
const syncFolderButton = document.querySelector<HTMLButtonElement>("#sync-folder-pick")!;
const navSyncDot = document.querySelector<HTMLElement>("#nav-sync-dot")!;
const vaultPasswordDialog = document.querySelector<HTMLDialogElement>("#vault-password")!;
const vaultPasswordForm = document.querySelector<HTMLFormElement>("#vault-password-form")!;
const vaultPasswordTitle = document.querySelector<HTMLElement>("#vault-password-title")!;
const vaultPasswordNote = document.querySelector<HTMLElement>("#vault-password-note")!;
const vaultPasswordInput = document.querySelector<HTMLInputElement>("#vault-password-input")!;
const vaultPasswordAgainField = document.querySelector<HTMLElement>("#vault-password-again-field")!;
const vaultPasswordAgain = document.querySelector<HTMLInputElement>("#vault-password-again")!;
const vaultPasswordHelp = document.querySelector<HTMLElement>("#vault-password-help")!;
const vaultPasswordError = document.querySelector<HTMLElement>("#vault-password-error")!;
const vaultPasswordAlt = document.querySelector<HTMLButtonElement>("#vault-password-alt")!;
const vaultPasswordSubmit = document.querySelector<HTMLButtonElement>("#vault-password-submit")!;

let snapshot: Snapshot = { tunnels: [], selectedId: null, configPath: "", notify: true };
let editingId: string | null = null;
type RowDrag = {
  id: string;
  row: HTMLElement;
  startY: number;
  /** Where inside the row the pointer grabbed it. */
  grab: number;
  active: boolean;
  pointerId: number;
};

let draggingRow: RowDrag | null = null;
let renderPending = false;
const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
const DRAG_SLIDE_MS = 160;
const DRAG_SETTLE_MS = 170;
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

/** Applies the newest snapshot only: a command's reply can arrive after a newer event. */
function applySnapshot(next: Snapshot) {
  if ((next.revision ?? 0) < (snapshot.revision ?? 0)) return;
  snapshot = next;
  render();
}

function render() {
  // A drag owns the rows while it runs; redraw once it is done.
  if (draggingRow) {
    renderPending = true;
    return;
  }
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
    row.dataset.id = tunnel.id;
    row.addEventListener("click", (event) => {
      if (event.target instanceof Element && event.target.closest("button:not(.row-main)")) return;
      void select(tunnel.id);
    });

    const handle = dragHandle(tunnel, row);

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

    row.append(handle, tile, main, detail, status, actions);
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

/** The grip at the left edge of a row; dragging it moves the tunnel. */
function dragHandle(tunnel: Tunnel, row: HTMLElement): HTMLElement {
  const handle = document.createElement("span");
  handle.className = "drag-handle";
  handle.setAttribute("aria-hidden", "true");
  handle.title = "拖动调整顺序";
  handle.addEventListener("click", (event) => event.stopPropagation());
  handle.addEventListener("pointerdown", (event) => beginRowDrag(event, tunnel.id, row));
  return handle;
}

function beginRowDrag(event: PointerEvent, id: string, row: HTMLElement) {
  if (event.button !== 0 || draggingRow) return;
  event.preventDefault();
  // Capture on the list, not the handle: reordering moves the row around,
  // and a capture target that leaves the document would end the drag.
  try {
    list.setPointerCapture(event.pointerId);
  } catch {
    // Without capture the drag still works while the pointer stays over the list.
  }
  const grab = event.clientY - row.getBoundingClientRect().top;
  draggingRow = { id, row, startY: event.clientY, grab, active: false, pointerId: event.pointerId };

  const move = (moveEvent: PointerEvent) => {
    const drag = draggingRow;
    if (!drag || drag.pointerId !== moveEvent.pointerId) return;
    if (!drag.active) {
      if (Math.abs(moveEvent.clientY - drag.startY) < 4) return;
      drag.active = true;
      row.style.transition = "none";
      row.classList.add("dragging");
      list.classList.add("sorting");
    }
    followPointer(drag, moveEvent.clientY);
  };

  const stop = (endEvent: PointerEvent) => {
    if (draggingRow && draggingRow.pointerId !== endEvent.pointerId) return;
    list.removeEventListener("pointermove", move);
    list.removeEventListener("pointerup", stop);
    list.removeEventListener("pointercancel", stop);
    list.removeEventListener("lostpointercapture", stop);
    const drag = draggingRow;
    draggingRow = null;
    if (!drag) return;
    if (list.hasPointerCapture(endEvent.pointerId)) list.releasePointerCapture(endEvent.pointerId);
    if (!drag.active) {
      if (renderPending) {
        renderPending = false;
        render();
      }
      return;
    }
    list.classList.remove("sorting");
    const after = row.nextElementSibling;
    const before = after instanceof HTMLElement && after.classList.contains("row") ? after.dataset.id ?? null : null;
    renderPending = false;
    const finish = () => {
      row.classList.remove("dragging");
      void invoke<Snapshot>("move_tunnel", { id: drag.id, before })
        .then((next) => applySnapshot(next))
        .catch((error: unknown) => {
          summary.textContent = errorText(error);
          summary.className = "info bad";
          void refresh();
        });
    };
    if (reducedMotion.matches) {
      row.style.transform = "";
      finish();
      return;
    }
    // Let the lifted row slide down into its slot before the list is redrawn.
    row.style.transition = `transform ${DRAG_SETTLE_MS}ms cubic-bezier(0.2, 0.7, 0.3, 1)`;
    row.style.transform = "translateY(0)";
    window.setTimeout(finish, DRAG_SETTLE_MS);
  };

  list.addEventListener("pointermove", move);
  list.addEventListener("pointerup", stop);
  list.addEventListener("pointercancel", stop);
  list.addEventListener("lostpointercapture", stop);
}

/**
 * Keeps the lifted row under the pointer and slides the other rows aside: their
 * layout moves at once, then a transform eases them into the new places (FLIP).
 */
function followPointer(drag: RowDrag, pointerY: number) {
  const rows = [...list.querySelectorAll<HTMLElement>(".row")];
  const tops = new Map(rows.map((el) => [el, el.offsetTop]));
  const rect = list.getBoundingClientRect();
  const pointer = pointerY - rect.top + list.scrollTop;

  const others = rows.filter((el) => el !== drag.row);
  const next = others.find((el) => pointer < el.offsetTop + el.offsetHeight / 2);
  if (next) {
    if (next.previousElementSibling !== drag.row) list.insertBefore(drag.row, next);
  } else if (list.lastElementChild !== drag.row) {
    list.append(drag.row);
  }

  if (!reducedMotion.matches) {
    for (const el of others) {
      const delta = (tops.get(el) ?? el.offsetTop) - el.offsetTop;
      if (delta === 0) continue;
      el.style.transition = "none";
      el.style.transform = `translateY(${delta}px)`;
      void el.offsetHeight;
      el.style.transition = `transform ${DRAG_SLIDE_MS}ms ease`;
      el.style.transform = "";
    }
  }

  const top = rect.top - list.scrollTop + drag.row.offsetTop;
  drag.row.style.transform = `translateY(${pointerY - drag.grab - top}px)`;
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
  const next = await invoke<Snapshot>("list_tunnels");
  tunnelsLoaded = true;
  applySnapshot(next);
}

async function select(id: string) {
  applySnapshot(await invoke<Snapshot>("select_tunnel", { id }));
}

const pendingTunnels = new Set<string>();

async function toggle(tunnel: Tunnel) {
  if (pendingTunnels.has(tunnel.id)) return;
  pendingTunnels.add(tunnel.id);
  const command = busy(tunnel.status) ? "stop_tunnel" : "start_tunnel";
  try {
    applySnapshot(await invoke<Snapshot>(command, { id: tunnel.id }));
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
    if (machine.displayName.toLowerCase() === key) return true;
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
      applySnapshot(await invoke<Snapshot>("set_config_path", { path: picked }));
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
    .then((next) => applySnapshot(next))
    .catch((error: unknown) => {
      notifyToggle.checked = snapshot.notify;
      settingsError.hidden = false;
      settingsError.textContent = errorText(error);
    });
});

const NEED_PASSWORD = "需要同步密码。";

type SyncPhase = "off" | "ready" | "locked" | "error";

type SyncDevice = {
  name: string;
  savedAt: number;
  thisMac: boolean;
};

type SyncStatus = {
  folder: string;
  available: boolean;
  enabled: boolean;
  label: string;
  phase: SyncPhase;
  message: string;
  lastSync: number;
  devices: SyncDevice[];
  minPasswordLen: number;
  maxPasswordLen: number;
  passwordSymbols: string;
};

type SyncNote = {
  tunnelId: string;
  tunnelName: string;
  kind: "loginKept" | "settingsReplaced" | "keptDeleted" | "revived";
  fields: string[];
};

type SyncReport = {
  from: string;
  added: number;
  updated: number;
  removed: number;
  notes: SyncNote[];
  uploaded: boolean;
};

type SyncEvent = {
  status: SyncStatus;
  report: SyncReport | null;
};

type CloudVault = {
  vaultId: string;
  label: string;
  devices: string[];
  savedAt: number;
};

let syncStatus: SyncStatus | null = null;
/** The last merge worth telling the user about, until they dismiss it. */
let syncReportShown: SyncReport | null = null;
let syncWorking = false;
let syncCommandError = "";

function relativeTime(seconds: number): string {
  if (!seconds) return "";
  const gap = Math.max(0, Date.now() / 1000 - seconds);
  if (gap < 60) return "刚刚";
  if (gap < 3600) return `${Math.floor(gap / 60)} 分钟前`;
  if (gap < 86400) return `${Math.floor(gap / 3600)} 小时前`;
  return new Date(seconds * 1000).toLocaleString("zh-CN", {
    month: "numeric",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** Shows iCloud Drive paths the way Finder names them. */
function prettyFolder(path: string): string {
  if (!path) return "找不到 iCloud 云盘";
  const cloud = path.match(/\/Library\/Mobile Documents\/com~apple~CloudDocs(\/.*)?$/);
  if (cloud) {
    const rest = (cloud[1] ?? "").split("/").filter(Boolean);
    return ["iCloud 云盘", ...rest].join(" › ");
  }
  return path.replace(/^\/Users\/[^/]+/, "~");
}

/** Swaps a button's icon for a spinner while its work runs. */
function setBusy(button: HTMLButtonElement, on: boolean) {
  const glyph = button.querySelector("i");
  if (on) {
    if (glyph && !button.dataset.icon) {
      button.dataset.icon = glyph.className;
      glyph.className = "ri-loader-4-line spin";
    }
    button.setAttribute("aria-busy", "true");
    button.disabled = true;
    return;
  }
  if (glyph && button.dataset.icon) glyph.className = button.dataset.icon;
  delete button.dataset.icon;
  button.removeAttribute("aria-busy");
  button.disabled = false;
}

function passwordRules() {
  return {
    min: syncStatus?.minPasswordLen ?? 10,
    max: syncStatus?.maxPasswordLen ?? 32,
    symbols: syncStatus?.passwordSymbols ?? "!@#$%^&*-_+=.?",
  };
}

/** The same rules the backend applies when a new password is set. */
function checkNewPassword(value: string): string | null {
  const { min, max, symbols } = passwordRules();
  if (!value) return "密码不能为空。";
  for (const char of value) {
    if (!/[A-Za-z0-9]/.test(char) && !symbols.includes(char)) {
      return `密码只能用字母、数字和这些符号：${symbols}`;
    }
  }
  if (value.length < min) return `密码至少要 ${min} 个字符。`;
  if (value.length > max) return `密码最多 ${max} 个字符。`;
  if (!/[A-Za-z]/.test(value) || !/[0-9]/.test(value)) return "密码要同时包含字母和数字。";
  return null;
}

type PasswordRequest = {
  title: string;
  note: string;
  /** A new password: check the rules and ask for it twice. */
  fresh: boolean;
  confirmLabel: string;
  /** A secondary way out, such as "start a new one instead". */
  alt?: string;
  /** Runs with the dialog still open, so a wrong password can be retyped. */
  action: (password: string) => Promise<void>;
};

type PasswordOutcome = "done" | "alt" | "cancel";

let passwordRequest: PasswordRequest | null = null;
let passwordSettle: ((outcome: PasswordOutcome) => void) | null = null;
let passwordWorking = false;

function paintVaultPasswordToggle(shown: boolean) {
  const button = document.querySelector<HTMLButtonElement>("#toggle-vault-password")!;
  button.replaceChildren(icon(shown ? "eye-off-line" : "eye-line"));
  button.setAttribute("aria-label", shown ? "隐藏密码" : "显示密码");
  button.title = shown ? "隐藏密码" : "显示密码";
}

function showPasswordError(text: string) {
  vaultPasswordError.textContent = text;
  vaultPasswordError.hidden = false;
}

function askPassword(request: PasswordRequest): Promise<PasswordOutcome> {
  return new Promise((resolve) => {
    passwordSettle?.("cancel");
    passwordRequest = request;
    passwordSettle = resolve;
    const { min, max, symbols } = passwordRules();
    vaultPasswordTitle.textContent = request.title;
    vaultPasswordNote.textContent = request.note;
    vaultPasswordError.hidden = true;
    vaultPasswordInput.value = "";
    vaultPasswordAgain.value = "";
    vaultPasswordInput.type = "password";
    vaultPasswordAgain.type = "password";
    paintVaultPasswordToggle(false);
    vaultPasswordAgainField.hidden = !request.fresh;
    vaultPasswordHelp.hidden = !request.fresh;
    vaultPasswordHelp.textContent = request.fresh
      ? `${min}–${max} 个字符，字母和数字都要有，符号只能用 ${symbols}。忘了它就没法在新的 Mac 上加入同步，请记好。`
      : "";
    vaultPasswordAlt.hidden = !request.alt;
    vaultPasswordAlt.textContent = request.alt ?? "";
    vaultPasswordSubmit.querySelector("span")!.textContent = request.confirmLabel;
    vaultPasswordDialog.showModal();
    vaultPasswordInput.focus();
  });
}

function finishPassword(outcome: PasswordOutcome) {
  const settle = passwordSettle;
  passwordSettle = null;
  passwordRequest = null;
  vaultPasswordInput.value = "";
  vaultPasswordAgain.value = "";
  if (vaultPasswordDialog.open) vaultPasswordDialog.close();
  settle?.(outcome);
}

vaultPasswordForm.addEventListener("submit", (event) => {
  event.preventDefault();
  const request = passwordRequest;
  if (!request || passwordWorking) return;
  const value = vaultPasswordInput.value;
  const problem = request.fresh
    ? checkNewPassword(value) ?? (value === vaultPasswordAgain.value ? null : "两次输入的密码不一样。")
    : value ? null : "请输入同步密码。";
  if (problem) {
    showPasswordError(problem);
    return;
  }
  passwordWorking = true;
  vaultPasswordError.hidden = true;
  setBusy(vaultPasswordSubmit, true);
  vaultPasswordAlt.disabled = true;
  void request.action(value)
    .then(() => finishPassword("done"))
    .catch((error: unknown) => {
      showPasswordError(errorText(error));
      vaultPasswordInput.select();
    })
    .finally(() => {
      passwordWorking = false;
      setBusy(vaultPasswordSubmit, false);
      vaultPasswordAlt.disabled = false;
    });
});

function cancelPassword() {
  if (!passwordWorking) finishPassword("cancel");
}

document.querySelector("#close-vault-password")!.addEventListener("click", cancelPassword);
document.querySelector("#vault-password-cancel")!.addEventListener("click", cancelPassword);
vaultPasswordAlt.addEventListener("click", () => {
  if (!passwordWorking) finishPassword("alt");
});
vaultPasswordDialog.addEventListener("cancel", (event) => {
  // 正在解密或加密时不让 Esc 关掉，免得结果回来时没地方显示。
  if (passwordWorking) event.preventDefault();
});
vaultPasswordDialog.addEventListener("close", () => {
  if (passwordSettle) finishPassword("cancel");
});
document.querySelector<HTMLButtonElement>("#toggle-vault-password")!.addEventListener("click", () => {
  const show = vaultPasswordInput.type === "password";
  vaultPasswordInput.type = show ? "text" : "password";
  vaultPasswordAgain.type = show ? "text" : "password";
  paintVaultPasswordToggle(show);
});

function syncNeedsAttention(): boolean {
  if (!syncStatus?.enabled) return false;
  return syncStatus.phase === "locked"
    || syncStatus.phase === "error"
    || Boolean(syncReportShown?.notes.length);
}

function noteText(note: SyncNote, from: string): string {
  const name = note.tunnelName;
  if (note.kind === "loginKept") {
    return `${name}：两台 Mac 都改过${note.fields.join("、")}，保留了这台 Mac 的。需要的话请手动改成一致。`;
  }
  if (note.kind === "settingsReplaced") {
    return `${name}：两台 Mac 同时改了转发设置，采用了「${from}」较新的那次。`;
  }
  if (note.kind === "keptDeleted") {
    return `${name}：「${from}」删除了它，但它在这台 Mac 上正在运行或改过，所以保留了。`;
  }
  return `${name}：这台 Mac 删除过它，但「${from}」之后又改了它，所以恢复了。`;
}

function paintSyncReport() {
  const report = syncReportShown;
  syncReport.replaceChildren();
  if (!report || !syncStatus?.enabled) {
    syncReport.hidden = true;
    return;
  }
  syncReport.hidden = false;
  syncReport.classList.toggle("warn", report.notes.length > 0);
  const head = document.createElement("div");
  head.className = "sync-report-head";
  const parts: string[] = [];
  if (report.added) parts.push(`新增 ${report.added} 条`);
  if (report.updated) parts.push(`更新 ${report.updated} 条`);
  if (report.removed) parts.push(`移除 ${report.removed} 条`);
  const line = document.createElement("p");
  line.textContent = parts.length
    ? `已合并「${report.from}」的改动：${parts.join("，")}。`
    : `已合并「${report.from}」的改动。`;
  const close = document.createElement("button");
  close.type = "button";
  close.className = "icon small";
  close.title = "知道了";
  close.setAttribute("aria-label", "知道了");
  close.append(icon("close-line"));
  close.addEventListener("click", () => {
    syncReportShown = null;
    paintSync();
  });
  head.append(icon(report.notes.length ? "error-warning-line" : "check-line"), line, close);
  syncReport.append(head);
  if (report.notes.length) {
    const notes = document.createElement("ul");
    for (const note of report.notes) {
      const item = document.createElement("li");
      item.textContent = noteText(note, report.from);
      notes.append(item);
    }
    syncReport.append(notes);
  }
}

function paintSync() {
  navSyncDot.hidden = !syncNeedsAttention();
  const status = syncStatus;
  if (!status) return;
  const phase: SyncPhase = status.enabled ? status.phase : "off";
  syncCard.dataset.phase = phase;
  syncGlyph.className = phase === "off" ? "ri-cloud-off-line" : "ri-cloud-line";
  syncState.className = `sync-state ${phase}`;
  syncState.textContent = phase === "off"
    ? status.available ? "未开启" : "未开启 · 找不到 iCloud 云盘"
    : phase === "locked"
      ? "需要输入同步密码"
      : phase === "error"
        ? "同步没有完成"
        : status.lastSync ? `已同步 · ${relativeTime(status.lastSync)}` : "已开启";
  syncState.title = status.enabled && status.lastSync
    ? `上次同步：${new Date(status.lastSync * 1000).toLocaleString("zh-CN")}`
    : "";

  syncEnableButton.hidden = phase !== "off";
  syncNowButton.hidden = phase === "off" || phase === "locked";
  syncUnlockButton.hidden = phase !== "locked";
  syncDisableButton.hidden = phase === "off";
  syncIntro.hidden = phase !== "off";
  for (const button of [syncEnableButton, syncNowButton, syncUnlockButton, syncDisableButton, syncFolderButton]) {
    if (!button.hasAttribute("aria-busy")) button.disabled = syncWorking;
  }

  syncFolder.textContent = prettyFolder(status.folder);
  syncFolder.title = status.folder;

  const problem = syncCommandError || (phase === "error" ? status.message : "");
  syncError.hidden = !problem;
  syncError.textContent = problem;

  syncDevices.replaceChildren();
  syncDevices.hidden = phase === "off" || status.devices.length === 0;
  for (const device of status.devices) {
    const item = document.createElement("li");
    item.className = "sync-device";
    const name = document.createElement("span");
    name.className = "sync-device-name";
    name.textContent = device.name;
    item.append(icon("macbook-line"), name);
    if (device.thisMac) {
      const here = document.createElement("span");
      here.className = "here";
      here.textContent = "这台 Mac";
      item.append(here);
    }
    const when = document.createElement("span");
    when.className = "sync-device-time";
    when.textContent = device.savedAt ? `${relativeTime(device.savedAt)}更新` : "";
    item.append(when);
    syncDevices.append(item);
  }
  if (status.devices.length > 0 && status.devices.every((device) => device.thisMac)) {
    const hint = document.createElement("li");
    hint.className = "sync-hint";
    hint.textContent = "在另一台 Mac 上打开 TailTunnel，开启同步并输入同一个密码，它就会出现在这里。";
    syncDevices.append(hint);
  }
  paintSyncReport();
}

function takeSyncEvent(event: SyncEvent) {
  syncStatus = event.status;
  const report = event.report;
  if (report?.from && (report.added || report.updated || report.removed || report.notes.length)) {
    syncReportShown = report;
  }
  if (!event.status.enabled) syncReportShown = null;
  paintSync();
}

async function refreshSync() {
  try {
    syncStatus = await invoke<SyncStatus>("sync_status");
    syncCommandError = "";
  } catch (error) {
    syncCommandError = errorText(error);
  }
  paintSync();
}

/** Runs one user action on the sync card. Returns whether it succeeded. */
async function runSyncAction(button: HTMLButtonElement, work: () => Promise<void>): Promise<boolean> {
  if (syncWorking) return false;
  syncWorking = true;
  syncCommandError = "";
  setBusy(button, true);
  paintSync();
  try {
    await work();
    return true;
  } catch (error) {
    // 缺密码会让卡片切到“需要输入同步密码”，不必再报错。
    if (errorText(error) !== NEED_PASSWORD) syncCommandError = errorText(error);
    return false;
  } finally {
    syncWorking = false;
    setBusy(button, false);
    paintSync();
  }
}

async function enableSync() {
  let vaults: CloudVault[] = [];
  const found = await runSyncAction(syncEnableButton, async () => {
    vaults = await invoke<CloudVault[]>("sync_inspect");
  });
  if (!found) return;
  const existing = vaults[0];
  if (existing) {
    const outcome = await askPassword({
      title: `加入「${existing.label || "我的隧道"}」`,
      note: `iCloud 里已有 ${existing.devices.join("、")} 同步的隧道配置。输入当时设的同步密码就能加入；这台 Mac 已有的隧道会合并进去，不会被删除。`,
      fresh: false,
      confirmLabel: "加入",
      alt: "不加入，新建一份",
      action: async (password) => {
        syncStatus = await invoke<SyncStatus>("sync_enable", { password, join: existing.vaultId });
        paintSync();
      },
    });
    if (outcome !== "alt") return;
  }
  await askPassword({
    title: "设置同步密码",
    note: existing
      ? "会新建一份独立的同步配置，和 iCloud 里已有的那份互不影响。"
      : "其他 Mac 加入同步时要输入这个密码。它只存在每台 Mac 自己的钥匙串里。",
    fresh: true,
    confirmLabel: "开启同步",
    action: async (password) => {
      syncStatus = await invoke<SyncStatus>("sync_enable", { password, join: null });
      paintSync();
    },
  });
}

syncEnableButton.addEventListener("click", () => void enableSync());

syncNowButton.addEventListener("click", () => {
  void runSyncAction(syncNowButton, async () => {
    syncStatus = await invoke<SyncStatus>("sync_now");
  }).then((done) => {
    if (done) flashLabel(syncNowButton, "已同步");
  });
});

syncUnlockButton.addEventListener("click", () => {
  void askPassword({
    title: "输入同步密码",
    note: "这台 Mac 的钥匙串里没有能用的同步密码。输入一次，之后会记在这台 Mac 的钥匙串里。",
    fresh: false,
    confirmLabel: "解锁",
    action: async (password) => {
      syncStatus = await invoke<SyncStatus>("sync_unlock", { password });
      paintSync();
    },
  });
});

syncDisableButton.addEventListener("click", () => {
  void (async () => {
    const confirmed = await confirm(
      "这台 Mac 会停止同步，本机的隧道保持不变；iCloud 里的加密文件和其他 Mac 不受影响。这台 Mac 记住的同步密码会从钥匙串删除。",
      { title: "关闭 iCloud 同步", kind: "warning", okLabel: "关闭同步", cancelLabel: "取消" },
    );
    if (!confirmed) return;
    await runSyncAction(syncDisableButton, async () => {
      syncStatus = await invoke<SyncStatus>("sync_disable");
      syncReportShown = null;
    });
  })();
});

syncFolderButton.addEventListener("click", () => {
  void (async () => {
    const picked = await open({
      title: "同步文件夹",
      directory: true,
      multiple: false,
      defaultPath: syncStatus?.folder || undefined,
    });
    if (typeof picked !== "string") return;
    if (syncStatus?.enabled) {
      const confirmed = await confirm(
        "换文件夹后，其他 Mac 也要选同一个文件夹才能继续同步。",
        { title: "更改同步位置", kind: "info", okLabel: "更改", cancelLabel: "取消" },
      );
      if (!confirmed) return;
    }
    await runSyncAction(syncFolderButton, async () => {
      syncStatus = await invoke<SyncStatus>("sync_set_folder", { folder: picked });
    });
  })().catch((error: unknown) => {
    syncCommandError = errorText(error);
    paintSync();
  });
});

void listen<SyncEvent>("sync-changed", (event) => takeSyncEvent(event.payload));
window.setInterval(() => {
  if (document.body.dataset.view === "settings") paintSync();
}, 30_000);
void refreshSync();

document.querySelector<HTMLButtonElement>("#create")!.addEventListener("click", () => openEditor());

type AppView = "tunnels" | "tailscale" | "settings";

type TailscaleMachine = {
  hostName: string;
  dnsName: string;
  displayName: string;
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
  if (view === "settings") void refreshSync();
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
    host.textContent = machine.displayName;
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
    name.textContent = machine.displayName;
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
        .map((machine) => `${machine.ipv4}\t${machine.online}\t${machine.displayName}\t${machine.hostName}\t${machine.dnsName}`)
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

deleteButton.addEventListener("click", async () => {
  const tunnel = tunnelById(editingId);
  if (!tunnel || deleteButton.disabled) return;
  const name = tunnel.note || `${tunnel.user}@${tunnel.host}`;
  deleteButton.disabled = true;
  try {
    const confirmed = await confirm(`删除隧道“${name}”？`, {
      title: "删除隧道",
      kind: "warning",
      okLabel: "删除",
      cancelLabel: "取消",
    });
    if (!confirmed || editingId !== tunnel.id) return;
    const current = tunnelById(tunnel.id);
    if (!current) return;
    if (busy(current.status)) {
      const message = current.status === "waiting"
        ? `隧道“${name}”正在等待重连。删除会取消自动重连，确定先停止再删除？`
        : `隧道“${name}”正在运行或连接。删除会关闭这条隧道，确定先关闭再删除？`;
      const stopConfirmed = await confirm(message, {
        title: "关闭并删除隧道",
        kind: "warning",
        okLabel: "停止并删除",
        cancelLabel: "取消",
      });
      if (!stopConfirmed || editingId !== tunnel.id) return;
      snapshot = await invoke<Snapshot>("stop_tunnel", { id: tunnel.id });
      render();
    }
    snapshot = await invoke<Snapshot>("delete_tunnel", { id: tunnel.id });
    editor.close();
    render();
  } catch (error) {
    showError(errorText(error));
  } finally {
    deleteButton.disabled = false;
  }
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
      editor.close();
      applySnapshot(next);
    })
    .catch((error: unknown) => showError(errorText(error)));
});

void listen<Snapshot>("tunnels-changed", (event) => {
  applySnapshot(event.payload);
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

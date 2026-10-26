import "@fontsource/red-hat-display/900.css";
import "@fontsource/red-hat-text/400.css";
import "@fontsource/red-hat-text/500.css";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
import { MAX_LOG_CHARS, statusLabel, type Forward, type LogLine, type LogPage, type Snapshot, type Tunnel, type TunnelStatus } from "./types";

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
const hostInput = document.querySelector<HTMLInputElement>("#host")!;
const portInput = document.querySelector<HTMLInputElement>("#ssh-port")!;
const userInput = document.querySelector<HTMLInputElement>("#user")!;
const passwordInput = document.querySelector<HTMLInputElement>("#password")!;

let snapshot: Snapshot = { tunnels: [], selectedId: null, configPath: "" };
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

function busy(status: TunnelStatus): boolean {
  return status === "running" || status === "connecting";
}

function render() {
  const count = snapshot.tunnels.length;
  const running = runningCount();
  const connecting = connectingCount();
  summary.textContent = count === 0
    ? "没有隧道"
    : running > 0
      ? `${running} 条在跑`
      : connecting > 0
        ? "正在连接"
        : "没有在跑的隧道";
  summary.className = running > 0 || connecting > 0 ? "info live" : "info";
  settingsPath.textContent = snapshot.configPath;
  syncDock();

  list.classList.toggle("arrive", !seenArrival);
  list.replaceChildren();
  if (count === 0) {
    const empty = document.createElement("p");
    empty.className = "empty";
    empty.textContent = "还没有隧道";
    list.append(empty);
    seenArrival = true;
    return;
  }

  const bar = document.createElement("div");
  bar.className = "select-bar";
  list.append(bar);

  for (const tunnel of snapshot.tunnels) {
    const row = document.createElement("div");
    row.className = tunnel.id === snapshot.selectedId ? "row selected" : "row";

    const main = document.createElement("button");
    main.type = "button";
    main.className = "row-main";
    const identity = document.createElement("span");
    identity.className = "identity";
    identity.textContent = `${tunnel.user}@${tunnel.host}`;
    main.append(identity);
    const peer = peerOf(tunnel.host);
    if (peer) {
      const badge = document.createElement("span");
      badge.className = peer.online ? "peer on" : "peer off";
      badge.textContent = peer.online ? "在线" : "离线";
      main.append(badge);
    }
    main.addEventListener("click", () => {
      void select(tunnel.id);
    });

    const detail = document.createElement("div");
    detail.className = "detail";
    fillForwards(detail, tunnel.forwards);
    detail.addEventListener("click", (event) => {
      if (event.target instanceof HTMLButtonElement) return;
      void select(tunnel.id);
    });

    const status = document.createElement("span");
    paintStatus(status, tunnel.status, tunnel.connectedAt);

    const action = document.createElement("button");
    action.type = "button";
    action.className = busy(tunnel.status) ? "stop" : "go";
    action.textContent = busy(tunnel.status) ? "停止" : "启动";
    action.addEventListener("click", () => {
      void toggle(tunnel);
    });

    const edit = document.createElement("button");
    edit.type = "button";
    edit.textContent = "编辑";
    edit.addEventListener("click", () => openEditor(tunnel));

    row.append(main, detail, status, action, edit);
    if (tunnel.message) {
      const message = document.createElement("span");
      message.className = "message";
      message.textContent = tunnel.message;
      row.append(message);
    }
    list.append(row);
  }

  const placeBar = () => {
    const selected = list.querySelector(".row.selected");
    if (!(selected instanceof HTMLElement)) {
      bar.hidden = true;
      return;
    }
    bar.hidden = false;
    bar.style.height = `${selected.offsetHeight}px`;
    bar.style.transform = `translateY(${selected.offsetTop}px)`;
  };
  placeBar();
  if (!seenArrival) {
    seenArrival = true;
    requestAnimationFrame(() => bar.classList.add("move"));
  } else {
    bar.classList.add("move");
  }
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
  const row = document.createElement("div");
  row.className = "forward-row";
  const local = field("本地端口", String(forward.localPort || ""), "number");
  const remoteHost = field("远端地址", forward.remoteHost, "text");
  const remote = field("远端端口", String(forward.remotePort || ""), "number");
  const remove = document.createElement("button");
  remove.type = "button";
  remove.className = "icon";
  remove.textContent = "×";
  remove.setAttribute("aria-label", "去掉这一行");
  remove.addEventListener("click", () => {
    if (forwardsEl.children.length === 1) return;
    row.remove();
    syncRemoveButtons();
  });
  row.append(local, remoteHost, remote, remove);
  forwardsEl.append(row);
  syncRemoveButtons();
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
  hostInput.value = tunnel?.host ?? "";
  portInput.value = String(tunnel?.port ?? 22);
  userInput.value = tunnel?.user ?? "";
  passwordInput.value = tunnel?.password ?? "";
  passwordInput.type = "password";
  document.querySelector<HTMLButtonElement>("#toggle-password")!.textContent = "显示";
  deleteButton.hidden = !tunnel;
  document.querySelector<HTMLButtonElement>("#copy-tunnel-edit")!.hidden = !tunnel;
  document.querySelector<HTMLButtonElement>("#export-tunnel-edit")!.hidden = !tunnel;
  document.querySelector<HTMLElement>("#host-picks")!.hidden = true;
  formError.hidden = true;
  forwardsEl.replaceChildren();
  const forwards = tunnel?.forwards.length ? tunnel.forwards : [blankForward()];
  for (const forward of forwards) addForwardRow(forward);
  editor.showModal();
  hostInput.focus();
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
  dockTitle.textContent = `${tunnel.user}@${tunnel.host}`;
  fillForwards(dockMeta, tunnel.forwards);
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

function fillForwards(target: HTMLElement, forwards: Forward[]) {
  target.replaceChildren();
  forwards.forEach((forward, index) => {
    if (index > 0) target.append(document.createTextNode("   "));
    const port = document.createElement("button");
    port.type = "button";
    port.className = "port";
    port.textContent = String(forward.localPort);
    port.setAttribute("aria-label", `用浏览器打开 localhost:${forward.localPort}`);
    port.addEventListener("click", () => {
      void invoke("open_local", { port: forward.localPort }).catch((error: unknown) => {
        summary.textContent = errorText(error);
        summary.className = "info bad";
      });
    });
    target.append(port, document.createTextNode(` → ${forward.remoteHost}:${forward.remotePort}`));
  });
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

async function copyTunnel(tunnel: Tunnel, button: HTMLButtonElement) {
  const label = button.dataset.label ?? button.textContent ?? "复制";
  button.dataset.label = label;
  try {
    await navigator.clipboard.writeText(copyText(tunnel));
    button.textContent = "已复制";
    window.setTimeout(() => {
      button.textContent = label;
    }, 1200);
  } catch {
    summary.textContent = "没有复制。";
    summary.className = "info bad";
  }
}

async function exportTunnel(tunnel: Tunnel, button: HTMLButtonElement) {
  const label = button.dataset.label ?? button.textContent ?? "导出";
  button.dataset.label = label;
  try {
    const picked = await save({
      title: "导出隧道",
      defaultPath: `${tunnel.host}.json`,
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    if (typeof picked !== "string") return;
    await invoke("export_tunnel", { id: tunnel.id, path: picked });
    button.textContent = "已导出";
    window.setTimeout(() => {
      button.textContent = label;
    }, 1200);
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

function showView(view: AppView) {
  document.body.dataset.view = view;
  document.querySelector<HTMLElement>("#view-tunnels")!.hidden = view !== "tunnels";
  document.querySelector<HTMLElement>("#view-tailscale")!.hidden = view !== "tailscale";
  document.querySelector<HTMLElement>("#view-settings")!.hidden = view !== "settings";
  document.querySelector("#nav-tunnels")!.classList.toggle("current", view === "tunnels");
  document.querySelector("#nav-tailscale")!.classList.toggle("current", view === "tailscale");
  document.querySelector("#nav-settings")!.classList.toggle("current", view === "settings");
  if (view === "tailscale") void refreshMachines(true);
}

function iconSvg(path: string): SVGSVGElement {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 16 16");
  svg.setAttribute("width", "14");
  svg.setAttribute("height", "14");
  svg.setAttribute("aria-hidden", "true");
  svg.innerHTML = path;
  return svg;
}

function copyGlyph(): SVGSVGElement {
  return iconSvg(
    '<rect x="5.2" y="5.2" width="8" height="8" fill="none" stroke="currentColor" stroke-width="1.4"/><path d="M2.6 10.4V2.6h7.8" fill="none" stroke="currentColor" stroke-width="1.4"/>',
  );
}

function checkGlyph(): SVGSVGElement {
  return iconSvg(
    '<path d="M3 8.2 6.2 11.4 13 4.6" fill="none" stroke="currentColor" stroke-width="1.6"/>',
  );
}

function paintTailscale(report: TailscaleReport) {
  const body = document.querySelector<HTMLElement>("#tailscale-body")!;
  body.replaceChildren();
  if (!report.installed || report.message) {
    const note = document.createElement("p");
    note.className = "notice";
    note.textContent = report.message || "读不到 Tailscale。";
    body.append(note);
    return;
  }
  const others = report.machines.filter((machine) => !machine.here);
  if (others.length === 0) {
    const note = document.createElement("p");
    note.className = "notice";
    note.textContent = "网里还没有其他机器。";
    body.append(note);
  }
  for (const machine of report.machines) {
      const row = document.createElement("div");
      row.className = machine.online ? "machine live" : "machine";
      const name = document.createElement("span");
      name.className = "machine-name";
      const host = document.createElement("span");
      host.className = "identity";
      host.textContent = machine.hostName;
      name.append(host);
      if (machine.here) {
        const here = document.createElement("button");
        here.type = "button";
        here.className = "here";
        here.textContent = "本机";
        name.append(here);
      }
      const address = document.createElement("span");
      address.className = "machine-address";
      const ip = document.createElement("span");
      ip.className = "detail";
      ip.textContent = machine.ipv4;
      const copy = document.createElement("button");
      copy.type = "button";
      copy.className = "copy";
      copy.setAttribute("aria-label", "复制");
      copy.append(copyGlyph());
      copy.disabled = machine.ipv4.length === 0;
      copy.addEventListener("click", () => {
        void navigator.clipboard.writeText(machine.ipv4).then(() => {
          copy.replaceChildren(checkGlyph());
          copy.setAttribute("aria-label", "已复制");
          window.setTimeout(() => {
            copy.replaceChildren(copyGlyph());
            copy.setAttribute("aria-label", "复制");
          }, 1200);
        });
      });
      address.append(ip, copy);
      const state = document.createElement("span");
      state.className = "machine-state";
      const presence = document.createElement("span");
      presence.className = machine.online ? "status running" : "status stopped";
      presence.textContent = machine.online ? "Online" : "Offline";
      state.append(presence);
      if (machine.path) {
        const path = document.createElement("span");
        path.className = machine.path.startsWith("DERP") ? "path derp" : "path direct";
        path.textContent = machine.path;
        state.append(path);
      }
      row.append(name, address, state);
      body.append(row);
    }
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
    note("正在读取");
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
    button.textContent = machine.path
      ? `${machine.hostName}   ${machine.ipv4}   ${machine.online ? "在线" : "离线"}   ${machine.path}`
      : `${machine.hostName}   ${machine.ipv4}   ${machine.online ? "在线" : "离线"}`;
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
    body.replaceChildren();
    const pending = document.createElement("p");
    pending.className = "notice";
    pending.textContent = "正在读取";
    body.append(pending);
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
      body.replaceChildren();
      const note = document.createElement("p");
      note.className = "notice";
      note.textContent = errorText(error);
      body.append(note);
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
document.querySelector<HTMLButtonElement>("#toggle-password")!.addEventListener("click", () => {
  const show = passwordInput.type === "password";
  passwordInput.type = show ? "text" : "password";
  document.querySelector<HTMLButtonElement>("#toggle-password")!.textContent = show ? "隐藏" : "显示";
});

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
    host: hostInput.value,
    port: Number(portInput.value),
    user: userInput.value,
    password: passwordInput.value,
    forwards: readForwards(),
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

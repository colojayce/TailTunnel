import "@fontsource/red-hat-display/900.css";
import "@fontsource/red-hat-text/400.css";
import "@fontsource/red-hat-text/500.css";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { forwardLabel, statusLabel, type Forward, type LogLine, type Snapshot, type Tunnel } from "./types";

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

function runningCount(): number {
  return snapshot.tunnels.filter((tunnel) => tunnel.status === "running").length;
}

function render() {
  const count = snapshot.tunnels.length;
  const running = runningCount();
  summary.textContent = count === 0 ? "没有隧道" : running === 0 ? "没有在跑的隧道" : `${running} 条在跑`;
  summary.className = running > 0 ? "info live" : "info";
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
    const lamp = document.createElement("span");
    lamp.className = `lamp ${tunnel.status}`;
    lamp.setAttribute("aria-hidden", "true");
    const text = document.createElement("span");
    text.className = "row-text";
    const identity = document.createElement("span");
    identity.className = "identity";
    identity.textContent = `${tunnel.user}@${tunnel.host}`;
    const detail = document.createElement("span");
    detail.className = "detail";
    detail.textContent = tunnel.forwards.map(forwardLabel).join("   ");
    text.append(identity, detail);
    if (tunnel.message) {
      const message = document.createElement("span");
      message.className = "message";
      message.textContent = tunnel.message;
      text.append(message);
    }
    main.append(lamp, text);
    main.addEventListener("click", () => {
      void select(tunnel.id);
    });

    const status = document.createElement("span");
    status.className = `status ${tunnel.status}`;
    status.textContent = statusLabel(tunnel.status);

    const action = document.createElement("button");
    action.type = "button";
    action.className = tunnel.status === "running" ? "stop" : "go";
    action.textContent = tunnel.status === "running" ? "停止" : "启动";
    action.addEventListener("click", () => {
      void toggle(tunnel);
    });

    const edit = document.createElement("button");
    edit.type = "button";
    edit.textContent = "编辑";
    edit.addEventListener("click", () => openEditor(tunnel));

    row.append(main, status, action, edit);
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
  render();
}

async function select(id: string) {
  snapshot = await invoke<Snapshot>("select_tunnel", { id });
  render();
}

async function toggle(tunnel: Tunnel) {
  const command = tunnel.status === "running" ? "stop_tunnel" : "start_tunnel";
  try {
    snapshot = await invoke<Snapshot>(command, { id: tunnel.id });
    render();
  } catch (error) {
    await refresh();
    summary.textContent = errorText(error);
    summary.className = "info bad";
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
  editorNote.textContent = tunnel?.status === "running"
    ? "下次启动才生效。"
    : "";
  hostInput.value = tunnel?.host ?? "";
  portInput.value = String(tunnel?.port ?? 22);
  userInput.value = tunnel?.user ?? "";
  passwordInput.value = tunnel?.password ?? "";
  passwordInput.type = "password";
  document.querySelector<HTMLButtonElement>("#toggle-password")!.textContent = "显示";
  deleteButton.hidden = !tunnel;
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
  const changed = snapshot.selectedId !== output.dataset.tunnelId || status !== shownStatus;
  output.dataset.tunnelId = snapshot.selectedId ?? "";
  shownStatus = status;
  const dock = output.closest(".dock");
  if (!tunnel) {
    dock?.classList.add("idle");
    dockTitle.textContent = "";
    dockMeta.textContent = "";
    dockStatus.textContent = "";
    dockStatus.className = "status stopped";
    output.textContent = "";
    return;
  }
  dock?.classList.remove("idle");
  dockTitle.textContent = `${tunnel.user}@${tunnel.host}`;
  dockMeta.textContent = tunnel.forwards.map(forwardLabel).join("   ");
  dockStatus.textContent = statusLabel(tunnel.status);
  dockStatus.className = `status ${tunnel.status}`;
  if (changed && snapshot.selectedId) {
    void invoke<string>("tunnel_log", { id: snapshot.selectedId }).then((text) => {
      output.textContent = text;
      output.scrollTop = output.scrollHeight;
    });
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
  ipv4: string;
  online: boolean;
  here: boolean;
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
  if (view === "tailscale") void loadTailscale();
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

async function loadTailscale() {
  const body = document.querySelector<HTMLElement>("#tailscale-body")!;
  body.replaceChildren();
  const pending = document.createElement("p");
  pending.className = "notice";
  pending.textContent = "正在读取";
  body.append(pending);
  try {
    const report = await invoke<TailscaleReport>("tailscale_status");
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
      state.className = machine.online ? "status running" : "status stopped";
      state.textContent = machine.online ? "Online" : "Offline";
      row.append(name, address, state);
      body.append(row);
    }
  } catch (error) {
    body.replaceChildren();
    const note = document.createElement("p");
    note.className = "notice";
    note.textContent = errorText(error);
    body.append(note);
  }
}

document.querySelector("#nav-tunnels")!.addEventListener("click", () => showView("tunnels"));
document.querySelector("#nav-tailscale")!.addEventListener("click", () => showView("tailscale"));
document.querySelector("#nav-settings")!.addEventListener("click", () => showView("settings"));
document.querySelector("#refresh-tailscale")!.addEventListener("click", () => {
  void loadTailscale();
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
  const current = output.textContent ?? "";
  if (current.endsWith(event.payload.line) && current.length > 0) return;
  const nearBottom = output.scrollHeight - output.scrollTop - output.clientHeight < 48;
  const prefix = current;
  output.textContent = prefix ? `${prefix}\n${event.payload.line}` : event.payload.line;
  if (nearBottom) output.scrollTop = output.scrollHeight;
});

void refresh().catch((error: unknown) => {
  summary.textContent = errorText(error);
  summary.className = "info bad";
});

export type Forward = {
  localPort: number;
  remoteHost: string;
  remotePort: number;
};

export type JumpHost = {
  host: string;
  port: number;
  user: string;
  password: string;
};

export type TunnelStatus = "stopped" | "connecting" | "running" | "failed";

export type Tunnel = {
  id: string;
  note: string;
  host: string;
  port: number;
  user: string;
  password: string;
  jump: JumpHost | null;
  forwards: Forward[];
  status: TunnelStatus;
  message: string | null;
  connectedAt: number | null;
};

export type Snapshot = {
  tunnels: Tunnel[];
  selectedId: string | null;
  configPath: string;
};

export type LogLine = {
  id: string;
  seq: number;
  line: string;
};

export type LogPage = {
  floor: number;
  lines: { seq: number; line: string }[];
};

export const MAX_LOG_CHARS = 80_000;

export function statusLabel(status: TunnelStatus): string {
  if (status === "connecting") return "正在连接";
  if (status === "running") return "运行中";
  if (status === "failed") return "连接失败";
  return "已停止";
}

export type Leg = {
  samples: (number | null)[];
  minMs: number | null;
  avgMs: number | null;
  maxMs: number | null;
  lost: number;
};

export type LatencyReport = {
  direct: Leg;
  jumpLocal: Leg | null;
  viaJump: Leg | null;
  overheadMs: number | null;
  jumpError: string | null;
  faster: "direct" | "jump" | null;
};

function fmt(value: number | null): string {
  return value === null ? "—" : `${value < 10 ? value.toFixed(1) : Math.round(value)} ms`;
}

function legText(leg: Leg): string {
  if (leg.avgMs === null) return "不通";
  const lost = leg.lost > 0 ? `，丢 ${leg.lost}/${leg.samples.length}` : "";
  return `${fmt(leg.avgMs)}（最小 ${fmt(leg.minMs)}${lost}）`;
}

export function latencyLines(report: LatencyReport, hasJump: boolean): string[] {
  const lines = [`直连 ${legText(report.direct)}`];
  if (!hasJump) return lines;
  if (report.jumpLocal) lines.push(`本机→跳板 ${legText(report.jumpLocal)}`);
  if (report.viaJump) lines.push(`经跳板 ${legText(report.viaJump)}`);
  if (report.jumpError) lines.push(`跳板失败：${report.jumpError}`);
  const direct = report.direct.avgMs;
  const jump = report.viaJump?.avgMs ?? null;
  if (report.faster && direct !== null && jump !== null) {
    const gap = fmt(Math.abs(direct - jump));
    lines.push(report.faster === "jump" ? `经跳板更快，快 ${gap}` : `直连更快，快 ${gap}`);
  }
  return lines;
}

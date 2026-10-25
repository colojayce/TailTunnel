export type Forward = {
  localPort: number;
  remoteHost: string;
  remotePort: number;
};

export type TunnelStatus = "stopped" | "connecting" | "running" | "failed";

export type Tunnel = {
  id: string;
  host: string;
  port: number;
  user: string;
  password: string;
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

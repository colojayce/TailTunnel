export type Forward = {
  localPort: number;
  remoteHost: string;
  remotePort: number;
};

export type TunnelStatus = "stopped" | "running" | "failed";

export type Tunnel = {
  id: string;
  host: string;
  port: number;
  user: string;
  password: string;
  forwards: Forward[];
  status: TunnelStatus;
  message: string | null;
};

export type Snapshot = {
  tunnels: Tunnel[];
  selectedId: string | null;
  configPath: string;
};

export type LogLine = {
  id: string;
  line: string;
};

export function statusLabel(status: TunnelStatus): string {
  if (status === "running") return "运行中";
  if (status === "failed") return "连接失败";
  return "已停止";
}

export function forwardLabel(forward: Forward): string {
  return `${forward.localPort} → ${forward.remoteHost}:${forward.remotePort}`;
}

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TailscaleMachine {
    pub host_name: String,
    pub ipv4: String,
    pub online: bool,
    pub here: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TailscaleReport {
    pub installed: bool,
    pub state: String,
    pub state_label: String,
    pub host_name: String,
    pub dns_name: String,
    pub ipv4: String,
    pub online_peers: u32,
    pub peer_count: u32,
    pub machines: Vec<TailscaleMachine>,
    pub message: String,
}

pub fn inspect() -> TailscaleReport {
    let Some(bin) = find_binary() else {
        return missing();
    };
    let output = Command::new(bin).arg("status").arg("--json").output();
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            return failed(format!("读不到 Tailscale 状态：{error}"));
        }
    };
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let err = err.trim();
        return failed(if err.is_empty() {
            "Tailscale 没有返回状态。".into()
        } else {
            format!("Tailscale：{err}")
        });
    }
    parse_status(&String::from_utf8_lossy(&output.stdout)).unwrap_or_else(failed)
}

fn missing() -> TailscaleReport {
    TailscaleReport {
        installed: false,
        state: "missing".into(),
        state_label: "未安装".into(),
        host_name: String::new(),
        dns_name: String::new(),
        ipv4: String::new(),
        online_peers: 0,
        peer_count: 0,
        machines: Vec::new(),
        message: "这台 Mac 上没有 Tailscale。连内网要靠它，这里先不提供安装。".into(),
    }
}

fn failed(message: String) -> TailscaleReport {
    TailscaleReport {
        installed: true,
        state: "error".into(),
        state_label: "读不到".into(),
        host_name: String::new(),
        dns_name: String::new(),
        ipv4: String::new(),
        online_peers: 0,
        peer_count: 0,
        machines: Vec::new(),
        message,
    }
}

pub fn parse_status(text: &str) -> Result<TailscaleReport, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("Tailscale 状态无法解析：{e}"))?;
    let state = value
        .get("BackendState")
        .and_then(|item| item.as_str())
        .unwrap_or("")
        .to_string();
    let node = value.get("Self");
    let host_name = node
        .and_then(|item| item.get("HostName"))
        .and_then(|item| item.as_str())
        .unwrap_or("")
        .to_string();
    let dns_name = node
        .and_then(|item| item.get("DNSName"))
        .and_then(|item| item.as_str())
        .unwrap_or("")
        .trim_end_matches('.')
        .to_string();
    let ipv4 = first_ipv4(node.and_then(|item| item.get("TailscaleIPs")))
        .or_else(|| first_ipv4(value.get("TailscaleIPs")))
        .unwrap_or_default();
    let peers = value.get("Peer").and_then(|item| item.as_object());
    let peer_count = peers.map(|map| map.len()).unwrap_or(0) as u32;
    let mut machines = Vec::new();
    if let Some(node) = node {
        machines.push(machine_from(node, true));
    }
    if let Some(map) = peers {
        for peer in map.values() {
            machines.push(machine_from(peer, false));
        }
    }
    machines.sort_by(|left, right| {
        right
            .here
            .cmp(&left.here)
            .then(right.online.cmp(&left.online))
            .then(left.host_name.cmp(&right.host_name))
    });
    let online_peers = machines.iter().filter(|machine| machine.online && !machine.here).count() as u32;
    Ok(TailscaleReport {
        installed: true,
        state_label: state_label(&state).into(),
        state,
        host_name,
        dns_name,
        ipv4,
        online_peers,
        peer_count,
        machines,
        message: String::new(),
    })
}

fn machine_from(value: &serde_json::Value, here: bool) -> TailscaleMachine {
    let host_name = value
        .get("HostName")
        .and_then(|item| item.as_str())
        .unwrap_or("");
    let dns_name = value
        .get("DNSName")
        .and_then(|item| item.as_str())
        .unwrap_or("")
        .trim_end_matches('.');
    let host_name = if !host_name.is_empty() {
        host_name.to_string()
    } else if !dns_name.is_empty() {
        dns_name.to_string()
    } else {
        "未命名".into()
    };
    TailscaleMachine {
        host_name,
        ipv4: first_ipv4(value.get("TailscaleIPs")).unwrap_or_default(),
        online: value.get("Online").and_then(|item| item.as_bool()) == Some(true),
        here,
    }
}

fn state_label(state: &str) -> &'static str {
    match state {
        "Running" => "已连接",
        "Stopped" => "已停止",
        "NeedsLogin" => "需要登录",
        "Starting" => "正在连接",
        _ => "没有状态",
    }
}

fn first_ipv4(value: Option<&serde_json::Value>) -> Option<String> {
    value?.as_array()?.iter().find_map(|item| {
        let text = item.as_str()?;
        text.contains('.').then(|| text.to_string())
    })
}

fn find_binary() -> Option<PathBuf> {
    let fixed = [
        "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
        "/opt/homebrew/bin/tailscale",
        "/usr/local/bin/tailscale",
    ];
    for candidate in fixed {
        let path = PathBuf::from(candidate);
        if path.is_file() {
            return Some(path);
        }
    }
    let path_var = std::env::var("PATH").unwrap_or_default();
    for dir in path_var.split(':') {
        let path = Path::new(dir).join("tailscale");
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_self_and_online_peers() {
        let report = parse_status(
            r#"{"BackendState":"Running","TailscaleIPs":["100.64.1.1","fd7a::1"],"Self":{"HostName":"mbp","DNSName":"mbp.ts.net.","Online":true,"TailscaleIPs":["100.64.1.1"]},"Peer":{"a":{"HostName":"nas","Online":true,"TailscaleIPs":["100.64.1.2"]},"b":{"HostName":"desk","Online":false,"TailscaleIPs":["100.64.1.3"]}}}"#,
        )
        .unwrap();
        assert!(report.installed);
        assert_eq!(report.state_label, "已连接");
        assert_eq!(report.machines.len(), 3);
        assert!(report.machines[0].here);
        assert_eq!(report.machines[1].host_name, "nas");
        assert_eq!(report.machines[1].ipv4, "100.64.1.2");
        assert!(report.machines[1].online);
        assert_eq!(report.machines[2].host_name, "desk");
        assert!(!report.machines[2].online);
    }
}

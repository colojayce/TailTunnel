use crate::model::{
    JumpHost, Tunnel, jump_prompt, probe_client_args, probe_master_args,
};
use serde::Serialize;
use std::io::Read;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

pub const SAMPLES: usize = 5;
const IO_TIMEOUT: Duration = Duration::from_secs(3);
const MASTER_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Leg {
    pub samples: Vec<Option<f64>>,
    pub min_ms: Option<f64>,
    pub avg_ms: Option<f64>,
    pub max_ms: Option<f64>,
    pub lost: usize,
}

impl Leg {
    pub fn from_samples(samples: Vec<Option<f64>>) -> Self {
        let ok: Vec<f64> = samples.iter().flatten().copied().collect();
        let lost = samples.len() - ok.len();
        let min_ms = ok.iter().copied().reduce(f64::min);
        let max_ms = ok.iter().copied().reduce(f64::max);
        let avg_ms = if ok.is_empty() {
            None
        } else {
            Some(ok.iter().sum::<f64>() / ok.len() as f64)
        };
        Self {
            samples,
            min_ms,
            avg_ms,
            max_ms,
            lost,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatencyReport {
    /// 本机直连目标：TCP 握手后到收到 SSH 横幅的时间。
    pub direct: Leg,
    /// 本机到跳板的 TCP 握手时间。
    pub jump_local: Option<Leg>,
    /// 经跳板到目标：开通道到收到目标 SSH 横幅的时间，已扣除本机起进程的开销。
    pub via_jump: Option<Leg>,
    pub overhead_ms: Option<f64>,
    pub jump_error: Option<String>,
    /// "direct" 或 "jump"，两边都有数据时给出平均值更低的一方。
    pub faster: Option<String>,
}

pub fn faster(direct: &Leg, via_jump: Option<&Leg>) -> Option<String> {
    let direct = direct.avg_ms?;
    let jump = via_jump?.avg_ms?;
    Some(if jump < direct { "jump" } else { "direct" }.to_string())
}

fn ms(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 1000.0 * 10.0).round() / 10.0
}

fn resolve(host: &str, port: u16) -> Result<std::net::SocketAddr, String> {
    (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("无法解析 {host}：{e}"))?
        .next()
        .ok_or_else(|| format!("无法解析 {host}"))
}

/// One TCP connect; returns (handshake time, time until the first byte arrived).
fn tcp_probe(addr: &std::net::SocketAddr, want_banner: bool) -> Option<(f64, Option<f64>)> {
    let start = Instant::now();
    let mut stream = TcpStream::connect_timeout(addr, IO_TIMEOUT).ok()?;
    let connected = start.elapsed();
    if !want_banner {
        return Some((ms(connected), None));
    }
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let mut byte = [0u8; 1];
    let banner = match stream.read(&mut byte) {
        Ok(1) => Some(ms(start.elapsed())),
        _ => None,
    };
    Some((ms(connected), banner))
}

pub fn probe_direct(host: &str, port: u16, count: usize) -> Result<Leg, String> {
    let addr = resolve(host, port)?;
    let samples = (0..count)
        .map(|_| tcp_probe(&addr, true).map(|(connect, banner)| banner.unwrap_or(connect)))
        .collect();
    Ok(Leg::from_samples(samples))
}

pub fn probe_connect(host: &str, port: u16, count: usize) -> Result<Leg, String> {
    let addr = resolve(host, port)?;
    let samples = (0..count)
        .map(|_| tcp_probe(&addr, false).map(|(connect, _)| connect))
        .collect();
    Ok(Leg::from_samples(samples))
}

pub struct ProbeEnv<'a> {
    pub askpass: &'a Path,
    pub jump_pw_file: &'a Path,
    pub known_hosts: &'a Path,
    pub socket: &'a Path,
}

fn ssh(args: &[String]) -> Command {
    let mut command = Command::new("ssh");
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn control_ok(jump: &JumpHost, socket: &Path, operation: &str) -> bool {
    ssh(&probe_client_args(jump, socket, Some(operation), None))
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

struct Master {
    child: Child,
}

impl Master {
    fn open(jump: &JumpHost, env: &ProbeEnv) -> Result<Self, String> {
        let _ = std::fs::remove_file(env.socket);
        let mut command = ssh(&probe_master_args(jump, env.known_hosts, env.socket));
        command
            .stderr(Stdio::piped())
            .env("SSH_ASKPASS", env.askpass)
            .env("SSH_ASKPASS_REQUIRE", "force")
            .env("DISPLAY", ":0")
            .env("TAILTUNNEL_PW_FILE", env.jump_pw_file)
            .env("TAILTUNNEL_JUMP_PROMPT", jump_prompt(jump))
            .env("TAILTUNNEL_JUMP_PW_FILE", env.jump_pw_file);
        let child = command.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "找不到 ssh。这台 Mac 上需要有 OpenSSH。".to_string()
            } else {
                format!("无法启动 ssh：{e}")
            }
        })?;
        let mut master = Self { child };
        let started = Instant::now();
        loop {
            if let Ok(Some(_)) = master.child.try_wait() {
                let mut text = String::new();
                if let Some(mut stderr) = master.child.stderr.take() {
                    let _ = stderr.read_to_string(&mut text);
                }
                let line = text
                    .lines()
                    .rev()
                    .find(|line| !line.trim().is_empty())
                    .unwrap_or("连接跳板失败");
                return Err(line.trim().to_string());
            }
            if control_ok(jump, env.socket, "check") {
                return Ok(master);
            }
            if started.elapsed() > MASTER_TIMEOUT {
                return Err("连接跳板超时。".into());
            }
            thread::sleep(Duration::from_millis(150));
        }
    }
}

impl Drop for Master {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.total_cmp(b));
    Some(values[values.len() / 2])
}

/// Time for a mux client to ask the master for a channel to `target` and get the
/// first byte back (the target's SSH banner).
fn channel_sample(jump: &JumpHost, socket: &Path, target: (&str, u16)) -> Option<f64> {
    let start = Instant::now();
    let mut command = ssh(&probe_client_args(jump, socket, None, Some(target)));
    command.stdout(Stdio::piped());
    let mut child = command.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut byte = [0u8; 1];
        let got = matches!(stdout.read(&mut byte), Ok(1));
        let _ = tx.send((got, Instant::now()));
    });
    let result = match rx.recv_timeout(IO_TIMEOUT + Duration::from_secs(2)) {
        Ok((true, at)) => Some(at.duration_since(start)),
        _ => None,
    };
    let _ = child.kill();
    let _ = child.wait();
    result.map(ms)
}

/// Process start-up cost of one mux client, measured with `-O check`, which only
/// talks to the local master and never touches the network.
fn client_overhead(jump: &JumpHost, socket: &Path, count: usize) -> Option<f64> {
    let mut times = Vec::new();
    for _ in 0..count {
        let start = Instant::now();
        if control_ok(jump, socket, "check") {
            times.push(ms(start.elapsed()));
        }
    }
    median(&mut times)
}

pub fn probe_via_jump(
    jump: &JumpHost,
    target: (&str, u16),
    env: &ProbeEnv,
    count: usize,
) -> Result<(Leg, Option<f64>), String> {
    let _master = Master::open(jump, env)?;
    let overhead = client_overhead(jump, env.socket, count);
    let cost = overhead.unwrap_or(0.0);
    let samples = (0..count)
        .map(|_| channel_sample(jump, env.socket, target).map(|raw| ms_floor(raw - cost)))
        .collect();
    let _ = control_ok(jump, env.socket, "exit");
    let _ = std::fs::remove_file(env.socket);
    Ok((Leg::from_samples(samples), overhead))
}

fn ms_floor(value: f64) -> f64 {
    ((value.max(0.0)) * 10.0).round() / 10.0
}

pub fn measure(tunnel: &Tunnel, env: &ProbeEnv, count: usize) -> Result<LatencyReport, String> {
    let direct = probe_direct(&tunnel.host, tunnel.port, count)?;
    let mut report = LatencyReport {
        direct,
        jump_local: None,
        via_jump: None,
        overhead_ms: None,
        jump_error: None,
        faster: None,
    };
    if let Some(jump) = &tunnel.jump {
        match probe_connect(&jump.host, jump.port, count) {
            Ok(leg) => report.jump_local = Some(leg),
            Err(error) => report.jump_error = Some(error),
        }
        if report.jump_error.is_none() {
            match probe_via_jump(jump, (&tunnel.host, tunnel.port), env, count) {
                Ok((leg, overhead)) => {
                    report.via_jump = Some(leg);
                    report.overhead_ms = overhead;
                }
                Err(error) => report.jump_error = Some(error),
            }
        }
        report.faster = faster(&report.direct, report.via_jump.as_ref());
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    #[test]
    fn leg_statistics_ignore_lost_samples() {
        let leg = Leg::from_samples(vec![Some(10.0), None, Some(30.0), Some(20.0)]);
        assert_eq!(leg.min_ms, Some(10.0));
        assert_eq!(leg.max_ms, Some(30.0));
        assert_eq!(leg.avg_ms, Some(20.0));
        assert_eq!(leg.lost, 1);
        let none = Leg::from_samples(vec![None, None]);
        assert_eq!(none.avg_ms, None);
        assert_eq!(none.lost, 2);
    }

    #[test]
    fn faster_needs_both_sides_and_picks_the_lower_average() {
        let direct = Leg::from_samples(vec![Some(80.0)]);
        let jump = Leg::from_samples(vec![Some(50.0)]);
        assert_eq!(faster(&direct, Some(&jump)).as_deref(), Some("jump"));
        let slow = Leg::from_samples(vec![Some(120.0)]);
        assert_eq!(faster(&direct, Some(&slow)).as_deref(), Some("direct"));
        assert_eq!(faster(&direct, None), None);
        let lost = Leg::from_samples(vec![None]);
        assert_eq!(faster(&direct, Some(&lost)), None);
    }

    #[test]
    fn median_picks_the_middle_value() {
        assert_eq!(median(&mut [3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&mut []), None);
    }

    #[test]
    fn direct_probe_reads_the_banner_and_connect_probe_does_not_need_one() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            for stream in listener.incoming().flatten().take(6) {
                let mut stream = stream;
                let _ = stream.write_all(b"SSH-2.0-test\r\n");
            }
        });
        let direct = probe_direct("127.0.0.1", port, 3).unwrap();
        assert_eq!(direct.lost, 0);
        assert!(direct.avg_ms.unwrap() < 1000.0);
        let connect = probe_connect("127.0.0.1", port, 3).unwrap();
        assert_eq!(connect.lost, 0);
    }

    #[test]
    fn closed_port_counts_as_lost() {
        let port = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let leg = probe_connect("127.0.0.1", port, 2).unwrap();
        assert_eq!(leg.lost, 2);
    }
}

/// Privileged helper binary for DrayTek VPN network operations.
///
/// Invoked via pkexec to create/destroy TUN devices, configure routing, and manage DNS.
/// Designed to be minimal (std-only, no external deps) for security.
use std::net::Ipv4Addr;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: draytek-vpn-helper <setup|teardown|check> [options]");
        return ExitCode::from(1);
    }

    let result = match args[1].as_str() {
        "setup" => cmd_setup(&args[2..]),
        "teardown" => cmd_teardown(&args[2..]),
        "check" => cmd_check(),
        other => {
            eprintln!("Unknown subcommand: {other}");
            Err("Unknown subcommand".into())
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::from(1)
        }
    }
}

// ── Argument parsing ──────────────────────────────────────────────────────────

struct SetupArgs {
    device: String,
    uid: u32,
    local_ip: Ipv4Addr,
    peer_ip: Ipv4Addr,
    mtu: u16,
    routes: Vec<String>,
    default_gw: Option<Ipv4Addr>,
    dns: Option<Ipv4Addr>,
    /// Address the TLS tunnel is carried over. Pinned to its pre-VPN route when
    /// `default_gw` is set.
    vpn_server: Option<Ipv4Addr>,
}

struct TeardownArgs {
    device: String,
    restore_dns: bool,
}

fn parse_setup_args(args: &[String]) -> Result<SetupArgs, Box<dyn std::error::Error>> {
    let mut device = None;
    let mut uid = None;
    let mut local_ip = None;
    let mut peer_ip = None;
    let mut mtu = None;
    let mut routes = Vec::new();
    let mut default_gw = None;
    let mut dns = None;
    let mut vpn_server = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--device" => {
                i += 1;
                device = Some(args.get(i).ok_or("--device requires a value")?.clone());
            }
            "--uid" => {
                i += 1;
                uid = Some(
                    args.get(i)
                        .ok_or("--uid requires a value")?
                        .parse::<u32>()?,
                );
            }
            "--local-ip" => {
                i += 1;
                local_ip = Some(
                    args.get(i)
                        .ok_or("--local-ip requires a value")?
                        .parse::<Ipv4Addr>()?,
                );
            }
            "--peer-ip" => {
                i += 1;
                peer_ip = Some(
                    args.get(i)
                        .ok_or("--peer-ip requires a value")?
                        .parse::<Ipv4Addr>()?,
                );
            }
            "--mtu" => {
                i += 1;
                mtu = Some(
                    args.get(i)
                        .ok_or("--mtu requires a value")?
                        .parse::<u16>()?,
                );
            }
            "--route" => {
                i += 1;
                routes.push(args.get(i).ok_or("--route requires a value")?.clone());
            }
            "--default-gw" => {
                i += 1;
                default_gw = Some(
                    args.get(i)
                        .ok_or("--default-gw requires a value")?
                        .parse::<Ipv4Addr>()?,
                );
            }
            "--dns" => {
                i += 1;
                dns = Some(
                    args.get(i)
                        .ok_or("--dns requires a value")?
                        .parse::<Ipv4Addr>()?,
                );
            }
            "--vpn-server" => {
                i += 1;
                vpn_server = Some(
                    args.get(i)
                        .ok_or("--vpn-server requires a value")?
                        .parse::<Ipv4Addr>()?,
                );
            }
            other => return Err(format!("Unknown option: {other}").into()),
        }
        i += 1;
    }

    Ok(SetupArgs {
        device: device.ok_or("--device is required")?,
        uid: uid.ok_or("--uid is required")?,
        local_ip: local_ip.ok_or("--local-ip is required")?,
        peer_ip: peer_ip.ok_or("--peer-ip is required")?,
        mtu: mtu.ok_or("--mtu is required")?,
        routes,
        default_gw,
        dns,
        vpn_server,
    })
}

fn parse_teardown_args(args: &[String]) -> Result<TeardownArgs, Box<dyn std::error::Error>> {
    let mut device = None;
    let mut restore_dns = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--device" => {
                i += 1;
                device = Some(args.get(i).ok_or("--device requires a value")?.clone());
            }
            "--restore-dns" => {
                restore_dns = true;
            }
            other => return Err(format!("Unknown option: {other}").into()),
        }
        i += 1;
    }

    Ok(TeardownArgs {
        device: device.ok_or("--device is required")?,
        restore_dns,
    })
}

// ── Validation ────────────────────────────────────────────────────────────────

fn validate_device_name(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    if name.is_empty() || name.len() > 15 {
        return Err(format!("Device name must be 1-15 characters, got '{name}'").into());
    }
    if !name.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return Err(format!("Device name must start with a letter: '{name}'").into());
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(format!("Device name must be alphanumeric: '{name}'").into());
    }
    Ok(())
}

fn validate_mtu(mtu: u16) -> Result<(), Box<dyn std::error::Error>> {
    if !(576..=9000).contains(&mtu) {
        return Err(format!("MTU must be 576-9000, got {mtu}").into());
    }
    Ok(())
}

fn validate_cidr(cidr: &str) -> Result<(), Box<dyn std::error::Error>> {
    let parts: Vec<&str> = cidr.split('/').collect();
    if parts.len() != 2 {
        return Err(format!("Invalid CIDR format: {cidr}").into());
    }
    parts[0]
        .parse::<Ipv4Addr>()
        .map_err(|e| format!("Invalid IP in CIDR '{cidr}': {e}"))?;
    let prefix: u8 = parts[1]
        .parse()
        .map_err(|e| format!("Invalid prefix in CIDR '{cidr}': {e}"))?;
    if prefix > 32 {
        return Err(format!("Prefix length must be 0-32, got {prefix}").into());
    }
    Ok(())
}

// ── Command execution ─────────────────────────────────────────────────────────

fn run_cmd(program: &str, args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("+ {program} {}", args.join(" "));
    let output = Command::new(program).args(args).output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "{program} {} failed (exit {}): {}",
            args.join(" "),
            output.status,
            stderr.trim()
        )
        .into());
    }
    Ok(())
}

// ── DNS helpers ──────────────────────────────────────────────────────────

/// Try to configure DNS via resolvectl (systemd-resolved).
/// Returns true on success, false if resolvectl is unavailable or fails.
fn try_resolvectl_dns_setup(device: &str, dns_ip: Ipv4Addr) -> bool {
    let dns_str = dns_ip.to_string();
    if run_cmd("resolvectl", &["dns", device, &dns_str]).is_err() {
        return false;
    }
    if run_cmd("resolvectl", &["domain", device, "~."]).is_err() {
        return false;
    }
    true
}

/// Configure DNS by writing directly to /etc/resolv.conf (requires root).
fn direct_dns_setup(dns_ip: Ipv4Addr) -> Result<(), Box<dyn std::error::Error>> {
    let resolv_path = "/etc/resolv.conf";
    let backup_path = "/run/draytek-vpn-resolv.bak";

    // Backup current resolv.conf
    if let Ok(current) = std::fs::read_to_string(resolv_path) {
        std::fs::write(backup_path, &current)
            .map_err(|e| format!("Failed to backup resolv.conf to {backup_path}: {e}"))?;
    }

    // Prepend our nameserver
    let existing = std::fs::read_to_string(resolv_path).unwrap_or_default();
    let new_content = format!("nameserver {dns_ip}\n{existing}");
    std::fs::write(resolv_path, new_content)
        .map_err(|e| format!("Failed to write {resolv_path}: {e}"))?;
    Ok(())
}

// ── Full-tunnel routing helpers ──────────────────────────────────────────

/// Records the host route that pins the VPN server to its pre-tunnel path.
///
/// That route lives on the *physical* interface, so deleting the TUN device
/// does not clean it up — teardown has to remove it explicitly, including
/// after a crash, which is why it is persisted rather than passed as an arg.
const SERVER_ROUTE_STATE: &str = "/run/draytek-vpn-hostroute";

/// Extract `via <gateway>` and `dev <iface>` from `ip route get` output.
///
/// Only the first line describes the route — `ip` appends further lines such as
/// `cache` — so trailing lines are deliberately ignored.
///
/// A destination on a directly attached network has no `via`, hence the Option.
fn parse_route_get(output: &str) -> Option<(Option<Ipv4Addr>, String)> {
    let tokens: Vec<&str> = output.lines().next()?.split_whitespace().collect();
    let after = |key: &str| {
        tokens
            .iter()
            .position(|t| *t == key)
            .and_then(|i| tokens.get(i + 1))
            .copied()
    };
    let via = after("via").and_then(|s| s.parse::<Ipv4Addr>().ok());
    let dev = after("dev")?.to_string();
    Some((via, dev))
}

/// Pin the VPN server to the route it uses right now, before the tunnel claims
/// the default route.
///
/// Without this the TLS connection carrying the tunnel would itself be routed
/// into the tunnel, which deadlocks the link the moment full-tunnel mode
/// engages. Must be called while the routing table still reflects pre-VPN state.
fn pin_server_route(server: Ipv4Addr, tun_device: &str) -> Result<(), Box<dyn std::error::Error>> {
    let server_str = server.to_string();
    let output = Command::new("ip")
        .args(["route", "get", &server_str])
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "`ip route get {server_str}` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }

    let route = String::from_utf8_lossy(&output.stdout);
    let (via, dev) = parse_route_get(&route)
        .ok_or_else(|| format!("Could not parse route to {server_str}: {}", route.trim()))?;

    if dev == tun_device {
        return Err(format!(
            "Route to VPN server {server_str} already points at {tun_device}; \
             refusing to pin the tunnel to itself"
        )
        .into());
    }

    let mut spec = vec![format!("{server_str}/32")];
    if let Some(gw) = via {
        spec.push("via".to_string());
        spec.push(gw.to_string());
    }
    spec.push("dev".to_string());
    spec.push(dev);

    // `replace` rather than `add` so a leftover route from a crashed session
    // is overwritten instead of failing the whole setup with EEXIST.
    let mut cmd_args = vec!["route", "replace"];
    cmd_args.extend(spec.iter().map(String::as_str));
    run_cmd("ip", &cmd_args)?;

    std::fs::write(SERVER_ROUTE_STATE, spec.join(" "))
        .map_err(|e| format!("Failed to record server route to {SERVER_ROUTE_STATE}: {e}"))?;

    eprintln!("Pinned VPN server route: {}", spec.join(" "));
    Ok(())
}

/// Remove the VPN server host route recorded by `pin_server_route`.
///
/// Best-effort: a failure here must not stop the rest of teardown.
fn unpin_server_route() {
    if !std::path::Path::new(SERVER_ROUTE_STATE).exists() {
        return;
    }

    match std::fs::read_to_string(SERVER_ROUTE_STATE) {
        Ok(spec) => {
            let args: Vec<&str> = spec.split_whitespace().collect();
            // Re-validate: only ever hand back a route spec of the shape we wrote.
            if args.is_empty() || validate_cidr(args[0]).is_err() {
                eprintln!("Warning: ignoring malformed server route state: '{spec}'");
            } else {
                let mut cmd_args = vec!["route", "del"];
                cmd_args.extend_from_slice(&args);
                if let Err(e) = run_cmd("ip", &cmd_args) {
                    eprintln!("Warning: failed to remove server host route: {e}");
                }
            }
        }
        Err(e) => eprintln!("Warning: failed to read {SERVER_ROUTE_STATE}: {e}"),
    }

    let _ = std::fs::remove_file(SERVER_ROUTE_STATE);
}

// ── Subcommands ───────────────────────────────────────────────────────────────

fn cmd_setup(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let setup = parse_setup_args(args)?;

    // Validate all inputs before executing anything
    validate_device_name(&setup.device)?;
    validate_mtu(setup.mtu)?;
    for route in &setup.routes {
        validate_cidr(route)?;
    }

    let uid_str = setup.uid.to_string();
    let local_ip_str = setup.local_ip.to_string();
    let peer_ip_str = setup.peer_ip.to_string();
    let mtu_str = setup.mtu.to_string();

    // 1. Remove a stale device left behind by a previous session
    if std::path::Path::new(&format!("/sys/class/net/{}", setup.device)).exists() {
        eprintln!("Note: removing stale {} device", setup.device);
        let _ = run_cmd(
            "ip",
            &["tuntap", "del", "dev", &setup.device, "mode", "tun"],
        );
    }

    // 2. Pin the VPN server's own traffic to the pre-tunnel path. Only matters
    //    in full-tunnel mode, and only works before any tunnel route exists.
    if setup.default_gw.is_some() {
        match setup.vpn_server {
            Some(server) => pin_server_route(server, &setup.device)?,
            None => eprintln!(
                "Warning: --default-gw without --vpn-server; the tunnel's own \
                 connection may be routed into the tunnel"
            ),
        }
    }

    // 3. Create TUN device owned by user
    run_cmd(
        "ip",
        &[
            "tuntap",
            "add",
            "dev",
            &setup.device,
            "mode",
            "tun",
            "user",
            &uid_str,
        ],
    )?;

    // 4. Configure IP address
    run_cmd(
        "ip",
        &[
            "addr",
            "add",
            &local_ip_str,
            "peer",
            &peer_ip_str,
            "dev",
            &setup.device,
        ],
    )?;

    // 5. Set MTU and bring up
    run_cmd("ip", &["link", "set", &setup.device, "mtu", &mtu_str, "up"])?;

    // 6. Add routes
    for route in &setup.routes {
        run_cmd("ip", &["route", "add", route, "dev", &setup.device])?;
    }

    // 7. Send all traffic through the tunnel.
    //
    //    Two /1 routes rather than a second `default`: they are more specific
    //    than any default route, so they win without depending on metrics and
    //    without disturbing the existing default (which stays as the fallback
    //    once the tunnel goes away). They also vanish with the TUN device.
    if let Some(gw) = setup.default_gw {
        let gw_str = gw.to_string();
        for half in ["0.0.0.0/1", "128.0.0.0/1"] {
            run_cmd(
                "ip",
                &["route", "add", half, "via", &gw_str, "dev", &setup.device],
            )?;
        }
        eprintln!("Full tunnel enabled: all traffic routed via {gw_str}");
    }

    // 8. DNS configuration
    if let Some(dns_ip) = setup.dns {
        if try_resolvectl_dns_setup(&setup.device, dns_ip) {
            eprintln!("DNS configured via resolvectl for {}", setup.device);
        } else {
            eprintln!("resolvectl not available or failed, falling back to /etc/resolv.conf");
            match direct_dns_setup(dns_ip) {
                Ok(()) => eprintln!("DNS configured via /etc/resolv.conf: {dns_ip}"),
                Err(e) => {
                    eprintln!("Warning: DNS configuration failed: {e} — continuing without DNS")
                }
            }
        }
    }

    eprintln!("Setup complete for {}", setup.device);
    Ok(())
}

fn cmd_check() -> Result<(), Box<dyn std::error::Error>> {
    let status = std::fs::read_to_string("/proc/self/status")
        .map_err(|e| format!("Failed to read /proc/self/status: {e}"))?;

    let cap_eff = status
        .lines()
        .find(|line| line.starts_with("CapEff:"))
        .ok_or("CapEff line not found in /proc/self/status")?;

    let hex_str = cap_eff
        .split_whitespace()
        .nth(1)
        .ok_or("Failed to parse CapEff value")?;

    let caps = u64::from_str_radix(hex_str.trim_start_matches("0x"), 16)
        .map_err(|e| format!("Failed to parse CapEff hex '{hex_str}': {e}"))?;

    const CAP_NET_ADMIN: u64 = 1 << 12;
    if caps & CAP_NET_ADMIN != 0 {
        eprintln!("capability check: CAP_NET_ADMIN is present in effective set (CapEff={hex_str})");
        Ok(())
    } else {
        eprintln!(
            "capability check: CAP_NET_ADMIN is NOT present in effective set (CapEff={hex_str})"
        );
        Err("CAP_NET_ADMIN not present".into())
    }
}

fn cmd_teardown(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let teardown = parse_teardown_args(args)?;

    validate_device_name(&teardown.device)?;

    // 1. Flush routes (best-effort)
    if let Err(e) = run_cmd("ip", &["route", "flush", "dev", &teardown.device]) {
        eprintln!("Warning: failed to flush routes: {e}");
    }

    // 2. Bring down interface (best-effort)
    if let Err(e) = run_cmd("ip", &["link", "set", &teardown.device, "down"]) {
        eprintln!("Warning: failed to bring down {}: {e}", teardown.device);
    }

    // 3. Delete TUN device
    if let Err(e) = run_cmd(
        "ip",
        &["tuntap", "del", "dev", &teardown.device, "mode", "tun"],
    ) {
        eprintln!("Warning: failed to delete {}: {e}", teardown.device);
    }

    // 4. Remove the VPN server host route from the physical interface
    unpin_server_route();

    // 5. Restore DNS (try both methods — safe no-ops if nothing to do)
    if teardown.restore_dns {
        // Try resolvectl revert (no-op if resolvectl wasn't used or device is gone)
        match Command::new("resolvectl")
            .args(["revert", &teardown.device])
            .output()
        {
            Ok(output) if output.status.success() => {
                eprintln!("+ Reverted DNS via resolvectl for {}", teardown.device);
            }
            _ => {
                // resolvectl not available or device already gone — that's fine
            }
        }

        // Restore /etc/resolv.conf backup if it exists (covers direct-write case)
        let backup_path = "/run/draytek-vpn-resolv.bak";
        let resolv_path = "/etc/resolv.conf";
        if std::path::Path::new(backup_path).exists() {
            match std::fs::read_to_string(backup_path) {
                Ok(original) => {
                    if let Err(e) = std::fs::write(resolv_path, original) {
                        eprintln!("Warning: failed to restore {resolv_path}: {e}");
                    } else {
                        eprintln!("+ Restored {resolv_path} from backup");
                        let _ = std::fs::remove_file(backup_path);
                    }
                }
                Err(e) => {
                    eprintln!("Warning: failed to read DNS backup {backup_path}: {e}");
                }
            }
        }
    }

    eprintln!("Teardown complete for {}", teardown.device);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_route_via_gateway() {
        let line = "8.8.8.8 via 192.168.1.1 dev enp44s0 src 192.168.1.10 uid 1000";
        let (via, dev) = parse_route_get(line).unwrap();
        assert_eq!(via, Some(Ipv4Addr::new(192, 168, 1, 1)));
        assert_eq!(dev, "enp44s0");
    }

    #[test]
    fn parses_on_link_route_without_gateway() {
        // A server on a directly attached network has no `via` hop.
        let line = "192.168.1.1 dev enp44s0 src 192.168.1.10 uid 1000";
        let (via, dev) = parse_route_get(line).unwrap();
        assert_eq!(via, None);
        assert_eq!(dev, "enp44s0");
    }

    #[test]
    fn ignores_trailing_lines_of_route_output() {
        // Real `ip route get` output puts "cache" on a following line.
        let output = "8.8.8.8 via 192.168.1.1 dev enp44s0 src 192.168.1.10 uid 1000 \n    cache \n";
        let (via, dev) = parse_route_get(output).unwrap();
        assert_eq!(via, Some(Ipv4Addr::new(192, 168, 1, 1)));
        assert_eq!(dev, "enp44s0");
    }

    #[test]
    fn rejects_route_output_without_device() {
        assert!(parse_route_get("unreachable 8.8.8.8").is_none());
        assert!(parse_route_get("").is_none());
    }

    #[test]
    fn parse_setup_args_accepts_vpn_server() {
        let args: Vec<String> = [
            "--device",
            "draytek0",
            "--uid",
            "1000",
            "--local-ip",
            "10.0.0.10",
            "--peer-ip",
            "10.0.0.1",
            "--mtu",
            "1280",
            "--default-gw",
            "10.0.0.1",
            "--vpn-server",
            "203.0.113.5",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        let setup = parse_setup_args(&args).unwrap();
        assert_eq!(setup.vpn_server, Some(Ipv4Addr::new(203, 0, 113, 5)));
        assert_eq!(setup.default_gw, Some(Ipv4Addr::new(10, 0, 0, 1)));
    }

    #[test]
    fn parse_setup_args_leaves_vpn_server_unset_when_absent() {
        let args: Vec<String> = [
            "--device",
            "draytek0",
            "--uid",
            "1000",
            "--local-ip",
            "10.0.0.10",
            "--peer-ip",
            "10.0.0.1",
            "--mtu",
            "1280",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        assert_eq!(parse_setup_args(&args).unwrap().vpn_server, None);
    }
}

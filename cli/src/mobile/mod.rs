//! `chrome-use mobile`: drive the desktop agent panel from a paired phone.
//! The gateway is the only thing a phone reaches; see
//! docs/superpowers/specs/2026-10-19-mobile-remote-design.md.

mod crypto;
mod gateway;
mod panel;
mod policy;
mod push;
mod store;

use serde_json::json;
use std::net::SocketAddr;
use store::{State, Store};

pub const DEFAULT_PORT: u16 = 17345;

const USAGE: &str = "\
Usage:
  chrome-use mobile serve [--bind ADDR] [--allow-all-interfaces] [--profile P] [--push-url URL]
  chrome-use mobile pair --url ws(s)://HOST[:PORT] [--name NAME] [--profile P] [--json]
  chrome-use mobile devices [--json]
  chrome-use mobile revoke <deviceId> [--json]
  chrome-use mobile push-test <deviceId> [--push-url URL]";

struct Args {
    positional: Vec<String>,
    options: Vec<(String, Option<String>)>,
}

impl Args {
    fn parse(raw: &[String], takes_value: &[&str]) -> Result<Self, String> {
        let mut positional = Vec::new();
        let mut options = Vec::new();
        let mut i = 0;
        while i < raw.len() {
            let arg = &raw[i];
            if let Some(flag) = arg.strip_prefix("--") {
                let (name, inline) = match flag.split_once('=') {
                    Some((n, v)) => (n.to_string(), Some(v.to_string())),
                    None => (flag.to_string(), None),
                };
                if takes_value.contains(&name.as_str()) {
                    let value = match inline {
                        Some(v) => v,
                        None => {
                            i += 1;
                            raw.get(i)
                                .cloned()
                                .ok_or_else(|| format!("--{name} needs a value"))?
                        }
                    };
                    options.push((name, Some(value)));
                } else {
                    options.push((name, None));
                }
            } else {
                positional.push(arg.clone());
            }
            i += 1;
        }
        Ok(Self {
            positional,
            options,
        })
    }
    fn value(&self, name: &str) -> Option<String> {
        self.options
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .and_then(|(_, v)| v.clone())
    }
    fn flag(&self, name: &str) -> bool {
        self.options.iter().any(|(n, _)| n == name)
    }
    fn only(&self, allowed: &[&str]) -> Result<(), String> {
        match self
            .options
            .iter()
            .find(|(n, _)| !allowed.contains(&n.as_str()))
        {
            Some((name, _)) => Err(format!("Unknown option --{name}\n{USAGE}")),
            None => Ok(()),
        }
    }
}

/// Run `chrome-use mobile <args>`; returns the process exit code.
pub fn run_cli(raw: &[String]) -> i32 {
    let json = raw.iter().any(|a| a == "--json");
    match run(raw) {
        Ok(()) => 0,
        Err(error) => {
            if json {
                println!("{}", json!({"success":false,"error":error}));
            } else {
                eprintln!("{error}");
            }
            1
        }
    }
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|_| "Cannot start the async runtime".into())
}

fn run(raw: &[String]) -> Result<(), String> {
    let Some(command) = raw.first() else {
        return Err(USAGE.into());
    };
    let rest = &raw[1..];
    match command.as_str() {
        "serve" => {
            let args = Args::parse(rest, &["bind", "profile", "push-url"])?;
            args.only(&["bind", "profile", "push-url", "allow-all-interfaces"])?;
            let bind = bind_address(args.value("bind"), args.flag("allow-all-interfaces"))?;
            let profile = args
                .value("profile")
                .map(|p| valid_profile(&p))
                .transpose()?;
            let push_url = push::url(args.value("push-url"));
            runtime()?.block_on(gateway::serve(gateway::Options {
                bind,
                profile,
                push_url,
            }))
        }
        "pair" => {
            let args = Args::parse(rest, &["url", "name", "profile"])?;
            args.only(&["url", "name", "profile", "json"])?;
            pair(&args)
        }
        "devices" => {
            let args = Args::parse(rest, &[])?;
            args.only(&["json"])?;
            devices(args.flag("json"))
        }
        "revoke" => {
            let args = Args::parse(rest, &[])?;
            args.only(&["json"])?;
            let [id] = args.positional.as_slice() else {
                return Err(format!("revoke needs one device id\n{USAGE}"));
            };
            let device = Store::open()?.revoke(id)?;
            if args.flag("json") {
                println!("{}", json!({"success":true,"revoked":device.id}));
            } else {
                println!(
                    "Revoked {} ({}). A running gateway drops its connection within a second.",
                    device.name, device.id
                );
            }
            Ok(())
        }
        "push-test" => {
            let args = Args::parse(rest, &["push-url"])?;
            args.only(&["push-url"])?;
            let [id] = args.positional.as_slice() else {
                return Err(format!("push-test needs one device id\n{USAGE}"));
            };
            let url = push::url(args.value("push-url"));
            let store = Store::open()?;
            runtime()?.block_on(push::test(&store, id, &url))?;
            println!("Test notification sent through {url}");
            Ok(())
        }
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(())
        }
        other => Err(format!("Unknown mobile command: {other}\n{USAGE}")),
    }
}

fn bind_address(raw: Option<String>, all_interfaces: bool) -> Result<SocketAddr, String> {
    let address: SocketAddr = match raw {
        None => SocketAddr::from(([127, 0, 0, 1], DEFAULT_PORT)),
        Some(raw) => match raw.parse() {
            Ok(address) => address,
            // A bare IP takes the default port.
            Err(_) => SocketAddr::new(
                raw.parse()
                    .map_err(|_| format!("--bind expects IP or IP:PORT, got {raw}"))?,
                DEFAULT_PORT,
            ),
        },
    };
    if address.ip().is_unspecified() && !all_interfaces {
        return Err(
            "Binding every interface exposes the gateway to every network this computer joins; add --allow-all-interfaces to confirm"
                .into(),
        );
    }
    if !address.ip().is_loopback() {
        eprintln!(
            "warning: the gateway listens on {address}, reachable from outside this computer. Every connection still needs a paired device's key."
        );
    }
    Ok(address)
}

fn valid_profile(profile: &str) -> Result<String, String> {
    if profile.is_empty()
        || profile.len() > 80
        || !profile
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("Invalid panel profile id".into());
    }
    Ok(profile.into())
}

/// The panel profile to bind a new device to: `--profile`, or the only one.
fn choose_profile(flag: Option<String>) -> Result<String, String> {
    if let Some(profile) = flag {
        return valid_profile(&profile);
    }
    let profiles = crate::agent_panel::panel_profiles()?;
    match profiles.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err("No agent panel profile exists yet. Open the chrome-use side panel in Chrome once, then pair again".into()),
        many => Err(format!(
            "Several Chrome profiles use the agent panel; choose one with --profile:\n  {}",
            many.join("\n  ")
        )),
    }
}

fn pair(args: &Args) -> Result<(), String> {
    let Some(url) = args.value("url") else {
        let mut message = String::from(
            "pair needs --url: the address your phone uses to reach `chrome-use mobile serve`.",
        );
        let candidates = local_addresses();
        if !candidates.is_empty() {
            message.push_str(
                "\nAddresses on this computer (start serve with --bind on that address first):",
            );
            for (ip, label) in candidates {
                message.push_str(&format!("\n  ws://{ip}:{DEFAULT_PORT}   {label}"));
            }
        }
        message.push_str("\nFor a Cloudflare named tunnel use its wss://… hostname. See https://chrome-use.leeguoo.com/mobile.html");
        return Err(message);
    };
    let parsed = url::Url::parse(&url).map_err(|_| "--url is not a valid URL")?;
    if !matches!(parsed.scheme(), "ws" | "wss") || parsed.host_str().is_none() {
        return Err("--url must be a ws:// or wss:// address".into());
    }
    if parsed
        .host_str()
        .is_some_and(|h| h.ends_with(".trycloudflare.com"))
    {
        eprintln!("note: quick tunnel URLs change on every run, which forces pairing again; use them only for testing.");
    }
    let name = args.value("name").unwrap_or_else(|| "Phone".into());
    if name.trim().is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
        return Err("--name must be 1-64 printable characters".into());
    }
    let profile = choose_profile(args.value("profile"))?;
    let store = Store::open()?;
    let (device, secret, gateway) = store.create_pending(name.trim(), &profile)?;
    let link = format!(
        "chromeuse://pair?v=1&url={}&id={}&k={}&fp={}",
        urlencoding::encode(&url),
        device.id,
        crypto::b64url(&secret),
        gateway
    );
    if args.flag("json") {
        println!(
            "{}",
            json!({"success":true,"deviceId":device.id,"profile":profile,"expires":device.expires,"pairUri":link})
        );
        return Ok(());
    }
    let code = qrcode::QrCode::new(link.as_bytes())
        .map_err(|_| "The pairing link is too long for a QR code")?;
    let image = code
        .render::<qrcode::render::unicode::Dense1x2>()
        .dark_color(qrcode::render::unicode::Dense1x2::Light)
        .light_color(qrcode::render::unicode::Dense1x2::Dark)
        .quiet_zone(true)
        .build();
    println!("{image}");
    println!("Scan with the chrome-use app within 10 minutes. It works once.");
    println!("Device: {} ({})", device.name, device.id);
    println!("Link (treat it like a password): {link}");
    Ok(())
}

fn devices(json_output: bool) -> Result<(), String> {
    let registry = Store::open()?.read()?;
    let now = store::now_ms();
    let rows: Vec<_> = registry
        .devices
        .iter()
        .map(|d| {
            let state = match d.state {
                State::Pending if !d.usable(now) => "expired",
                State::Pending => "pending",
                State::Active => "active",
                State::Disabled => "disabled",
            };
            json!({"id":d.id,"name":d.name,"state":state,"profile":d.profile,"created":d.created,"lastSeen":d.last_seen,"lastIp":d.last_ip,"push":d.relay_token.is_some()})
        })
        .collect();
    if json_output {
        println!(
            "{}",
            json!({"success":true,"gateway":registry.gateway_id,"devices":rows})
        );
        return Ok(());
    }
    if rows.is_empty() {
        println!("No paired devices. Pair one with `chrome-use mobile pair --url …`.");
        return Ok(());
    }
    let when = |ms: Option<u64>| match ms
        .and_then(|ms| chrono::DateTime::from_timestamp_millis(ms as i64))
    {
        Some(t) => t
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
        None => "never".into(),
    };
    for (row, device) in rows.iter().zip(&registry.devices) {
        println!(
            "{}  {}  [{}]  created {}  last seen {}{}{}",
            device.id,
            device.name,
            row["state"].as_str().unwrap_or(""),
            when(Some(device.created)),
            when(device.last_seen),
            device
                .last_ip
                .as_ref()
                .map(|ip| format!(" from {ip}"))
                .unwrap_or_default(),
            if device.relay_token.is_some() {
                "  push on"
            } else {
                ""
            }
        );
    }
    Ok(())
}

/// Non-loopback IPv4 addresses, labelled; Tailscale uses 100.64.0.0/10.
fn local_addresses() -> Vec<(std::net::Ipv4Addr, &'static str)> {
    let mut found = Vec::new();
    #[cfg(unix)]
    unsafe {
        let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut list) == 0 {
            let mut cursor = list;
            while !cursor.is_null() {
                let entry = &*cursor;
                if !entry.ifa_addr.is_null() && (*entry.ifa_addr).sa_family as i32 == libc::AF_INET
                {
                    let addr = &*(entry.ifa_addr as *const libc::sockaddr_in);
                    let ip = std::net::Ipv4Addr::from(u32::from_be(addr.sin_addr.s_addr));
                    if !ip.is_loopback()
                        && !ip.is_link_local()
                        && !found.iter().any(|(f, _)| *f == ip)
                    {
                        let octets = ip.octets();
                        let label = if octets[0] == 100 && (64..128).contains(&octets[1]) {
                            "Tailscale"
                        } else if ip.is_private() {
                            "LAN"
                        } else {
                            "public"
                        };
                        found.push((ip, label));
                    }
                }
                cursor = entry.ifa_next;
            }
            libc::freeifaddrs(list);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_every_interface_needs_confirmation() {
        assert!(bind_address(Some("0.0.0.0:9".into()), false).is_err());
        assert!(bind_address(Some("0.0.0.0:9".into()), true).is_ok());
        assert_eq!(
            bind_address(None, false).unwrap(),
            SocketAddr::from(([127, 0, 0, 1], DEFAULT_PORT))
        );
        assert_eq!(
            bind_address(Some("100.101.1.2".into()), false)
                .unwrap()
                .port(),
            DEFAULT_PORT
        );
    }

    #[test]
    fn options_are_parsed_strictly() {
        let raw: Vec<String> = ["--url", "wss://a", "--name=Leo phone", "x", "--json"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let args = Args::parse(&raw, &["url", "name"]).unwrap();
        assert_eq!(args.value("url").unwrap(), "wss://a");
        assert_eq!(args.value("name").unwrap(), "Leo phone");
        assert_eq!(args.positional, ["x"]);
        assert!(args.flag("json"));
        assert!(args.only(&["url", "name"]).is_err());
        assert!(Args::parse(&["--url".to_string()], &["url"]).is_err());
    }
}

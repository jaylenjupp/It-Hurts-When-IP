use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::process::Command;
use std::fs;


fn main() {
    let socket_path = "/var/run/com.ithurtswhenip.helper.sock";

    if std::path::Path::new(socket_path).exists() {
        fs::remove_file(socket_path).ok();
    }

    let listener = UnixListener::bind(socket_path).expect("Failed to bind socket");

    Command::new("chmod")
        .args(["777", socket_path])
        .output()
        .ok();

    println!("Helper running, listening on {}", socket_path);

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                std::thread::spawn(move || {
                    handle_client(stream);
                });
            }
            Err(e) => {
                eprintln!("Connection error: {}", e);
            }
        }
    }
}

fn handle_client(stream: std::os::unix::net::UnixStream) {
    let mut reader = BufReader::new(&stream);
    let mut writer = &stream;
    let mut line = String::new();

    if reader.read_line(&mut line).is_ok() {
        let request: serde_json::Value = match serde_json::from_str(line.trim()) {
            Ok(v) => v,
            Err(_) => {
                let _ = writer.write_all(b"{\"error\": \"invalid request\"}\n");
                return;
            }
        };

        let response = handle_request(&request);
        let _ = writer.write_all(format!("{}\n", response).as_bytes());
    }
}

fn handle_request(request: &serde_json::Value) -> String {
    let cmd = match request["cmd"].as_str() {
        Some(c) => c,
        None => return serde_json::json!({"error": "missing cmd"}).to_string(),
    };

    match cmd {
        "set_static_ip" => {
            let interface = request["interface"].as_str().unwrap_or("");
            let ip = request["ip"].as_str().unwrap_or("");
            let subnet = request["subnet"].as_str().unwrap_or("");
            let gateway = request["gateway"].as_str().unwrap_or("");

            let output = Command::new("networksetup")
                .args(["-setmanual", interface, ip, subnet, gateway])
                .output();

            match output {
                Ok(o) if o.status.success() => {
                    serde_json::json!({"ok": format!("Set {} to {}", interface, ip)}).to_string()
                }
                Ok(o) => {
                    let err = String::from_utf8_lossy(&o.stderr).to_string();
                    serde_json::json!({"error": err}).to_string()
                }
                Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
            }
        }

        "set_dhcp" => {
            let interface = request["interface"].as_str().unwrap_or("");

            let output = Command::new("networksetup")
                .args(["-setdhcp", interface])
                .output();

            match output {
                Ok(o) if o.status.success() => {
                    serde_json::json!({"ok": format!("Set {} to DHCP", interface)}).to_string()
                }
                Ok(o) => {
                    let err = String::from_utf8_lossy(&o.stderr).to_string();
                    serde_json::json!({"error": err}).to_string()
                }
                Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
            }
        }

        _ => serde_json::json!({"error": "unknown command"}).to_string(),
    }
}
use agentos_interfaces::ChannelError;
use std::env;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) struct WebSocketConnection {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ParsedWsUrl {
    host: String,
    port: u16,
    path_and_query: String,
}

impl WebSocketConnection {
    pub(super) fn connect(url: &str) -> Result<Self, ChannelError> {
        let parsed =
            ParsedWsUrl::parse(url).map_err(|err| ChannelError::Backend(Arc::from(err)))?;
        let proxy = https_proxy_for_openssl(&parsed.host);
        let mut command = Command::new("openssl");
        command
            .arg("s_client")
            .arg("-quiet")
            .arg("-connect")
            .arg(format!("{}:{}", parsed.host, parsed.port))
            .arg("-servername")
            .arg(&parsed.host)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(proxy) = &proxy {
            command.arg("-proxy").arg(proxy);
        }
        let mut child = command
            .spawn()
            .map_err(|err| ChannelError::Backend(Arc::from(err.to_string())))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| ChannelError::Backend(Arc::from("openssl stdin unavailable")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ChannelError::Backend(Arc::from("openssl stdout unavailable")))?;
        let mut stdout = BufReader::new(stdout);
        let key = websocket_key()?;
        write!(
            stdin,
            "GET {} HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {}\r\nSec-WebSocket-Version: 13\r\n\r\n",
            parsed.path_and_query, parsed.host, key
        )
        .map_err(|err| ChannelError::Backend(Arc::from(err.to_string())))?;
        stdin
            .flush()
            .map_err(|err| ChannelError::Backend(Arc::from(err.to_string())))?;

        let mut status = String::new();
        stdout
            .read_line(&mut status)
            .map_err(|err| ChannelError::Backend(Arc::from(err.to_string())))?;
        if !status.contains(" 101 ") {
            return Err(ChannelError::Backend(Arc::from(format!(
                "Feishu WebSocket upgrade failed: {}",
                status.trim()
            ))));
        }
        loop {
            let mut line = String::new();
            stdout
                .read_line(&mut line)
                .map_err(|err| ChannelError::Backend(Arc::from(err.to_string())))?;
            if line == "\r\n" || line == "\n" || line.is_empty() {
                break;
            }
        }

        Ok(Self {
            _child: child,
            stdin,
            stdout,
        })
    }

    pub(super) fn read_frame(&mut self) -> Result<Vec<u8>, ChannelError> {
        loop {
            let mut header = [0_u8; 2];
            self.stdout
                .read_exact(&mut header)
                .map_err(|err| ChannelError::Backend(Arc::from(err.to_string())))?;
            let opcode = header[0] & 0x0f;
            let masked = header[1] & 0x80 != 0;
            let mut len = u64::from(header[1] & 0x7f);
            if len == 126 {
                let mut bytes = [0_u8; 2];
                self.stdout
                    .read_exact(&mut bytes)
                    .map_err(|err| ChannelError::Backend(Arc::from(err.to_string())))?;
                len = u64::from(u16::from_be_bytes(bytes));
            } else if len == 127 {
                let mut bytes = [0_u8; 8];
                self.stdout
                    .read_exact(&mut bytes)
                    .map_err(|err| ChannelError::Backend(Arc::from(err.to_string())))?;
                len = u64::from_be_bytes(bytes);
            }
            let mask = if masked {
                let mut mask = [0_u8; 4];
                self.stdout
                    .read_exact(&mut mask)
                    .map_err(|err| ChannelError::Backend(Arc::from(err.to_string())))?;
                Some(mask)
            } else {
                None
            };
            if len > 16 * 1024 * 1024 {
                return Err(ChannelError::Backend(Arc::from(
                    "Feishu WebSocket frame is too large",
                )));
            }
            let mut payload = vec![0_u8; len as usize];
            self.stdout
                .read_exact(&mut payload)
                .map_err(|err| ChannelError::Backend(Arc::from(err.to_string())))?;
            if let Some(mask) = mask {
                for (index, byte) in payload.iter_mut().enumerate() {
                    *byte ^= mask[index % 4];
                }
            }
            match opcode {
                0x2 => return Ok(payload),
                0x8 => {
                    return Err(ChannelError::Backend(Arc::from(
                        "Feishu WebSocket closed by server",
                    )))
                }
                0x9 => {
                    self.write_control_frame(0x0a, &payload)?;
                }
                0x0a => {}
                _ => {}
            }
        }
    }

    pub(super) fn write_frame(&mut self, payload: &[u8]) -> Result<(), ChannelError> {
        self.write_ws_frame(0x2, payload)
    }

    fn write_control_frame(&mut self, opcode: u8, payload: &[u8]) -> Result<(), ChannelError> {
        self.write_ws_frame(opcode, payload)
    }

    fn write_ws_frame(&mut self, opcode: u8, payload: &[u8]) -> Result<(), ChannelError> {
        let mut frame = Vec::new();
        frame.push(0x80 | opcode);
        let mask_key = websocket_mask()?;
        if payload.len() < 126 {
            frame.push(0x80 | payload.len() as u8);
        } else if payload.len() <= u16::MAX as usize {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        } else {
            frame.push(0x80 | 127);
            frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        }
        frame.extend_from_slice(&mask_key);
        for (index, byte) in payload.iter().enumerate() {
            frame.push(byte ^ mask_key[index % 4]);
        }
        self.stdin
            .write_all(&frame)
            .and_then(|_| self.stdin.flush())
            .map_err(|err| ChannelError::Backend(Arc::from(err.to_string())))
    }
}

impl ParsedWsUrl {
    fn parse(input: &str) -> Result<Self, String> {
        let rest = input
            .strip_prefix("wss://")
            .ok_or_else(|| "Feishu WebSocket URL must use wss://".to_owned())?;
        let (authority, path) = rest
            .split_once('/')
            .map_or((rest, "/"), |(authority, path)| (authority, path));
        let (host, port) = if let Some((host, port)) = authority.rsplit_once(':') {
            (
                host.to_owned(),
                port.parse::<u16>()
                    .map_err(|err| format!("invalid WebSocket port: {err}"))?,
            )
        } else {
            (authority.to_owned(), 443)
        };
        if host.is_empty() {
            return Err("Feishu WebSocket URL is missing a host".to_owned());
        }
        Ok(Self {
            host,
            port,
            path_and_query: format!("/{path}"),
        })
    }
}

fn websocket_key() -> Result<String, ChannelError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| ChannelError::Backend(Arc::from(err.to_string())))?
        .as_nanos();
    Ok(base64_encode(&now.to_be_bytes()))
}

fn websocket_mask() -> Result<[u8; 4], ChannelError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| ChannelError::Backend(Arc::from(err.to_string())))?
        .as_nanos();
    let bytes = now.to_be_bytes();
    Ok([bytes[12], bytes[13], bytes[14], bytes[15]])
}

fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::new();
    for chunk in input.chunks(3) {
        let a = chunk[0];
        let b = *chunk.get(1).unwrap_or(&0);
        let c = *chunk.get(2).unwrap_or(&0);
        output.push(TABLE[(a >> 2) as usize] as char);
        output.push(TABLE[(((a & 0x03) << 4) | (b >> 4)) as usize] as char);
        if chunk.len() > 1 {
            output.push(TABLE[(((b & 0x0f) << 2) | (c >> 6)) as usize] as char);
        } else {
            output.push('=');
        }
        if chunk.len() > 2 {
            output.push(TABLE[(c & 0x3f) as usize] as char);
        } else {
            output.push('=');
        }
    }
    output
}

fn https_proxy_for_openssl(host: &str) -> Option<String> {
    if no_proxy_matches(host) {
        return None;
    }
    env::var("HTTPS_PROXY")
        .or_else(|_| env::var("https_proxy"))
        .ok()
        .and_then(|proxy| proxy.strip_prefix("http://").map(ToOwned::to_owned))
        .and_then(|proxy| {
            let proxy = proxy.trim_end_matches('/').to_owned();
            (!proxy.is_empty()).then_some(proxy)
        })
}

fn no_proxy_matches(host: &str) -> bool {
    env::var("NO_PROXY")
        .or_else(|_| env::var("no_proxy"))
        .ok()
        .is_some_and(|no_proxy| {
            no_proxy
                .split(',')
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .any(|entry| no_proxy_entry_matches(host, entry))
        })
}

fn no_proxy_entry_matches(host: &str, entry: &str) -> bool {
    entry == "*"
        || host.eq_ignore_ascii_case(entry)
        || entry
            .strip_prefix('.')
            .is_some_and(|suffix| host.ends_with(suffix))
}

use agentos_interfaces::ChannelError;
use std::env;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Maximum number of bytes drained from `openssl s_client`'s stderr when a
/// failure occurs. Capped so a chatty subprocess can't balloon error messages.
const STDERR_CAPTURE_LIMIT: u64 = 4 * 1024;

pub(super) struct WebSocketConnection {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    stderr: Option<ChildStderr>,
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
            // Pipe stderr so handshake / TLS / DNS errors surface in the
            // returned ChannelError instead of disappearing into /dev/null.
            .stderr(Stdio::piped());
        if let Some(proxy) = &proxy {
            command.arg("-proxy").arg(proxy);
        }
        let mut child = command
            .spawn()
            .map_err(|err| ChannelError::Backend(Arc::from(err.to_string())))?;
        let mut stderr = child.stderr.take();
        let mut stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => {
                return Err(finalize_failure(
                    &mut child,
                    stderr.take(),
                    "openssl stdin unavailable".to_owned(),
                ))
            }
        };
        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                return Err(finalize_failure(
                    &mut child,
                    stderr.take(),
                    "openssl stdout unavailable".to_owned(),
                ))
            }
        };
        let mut stdout = BufReader::new(stdout);
        let key = websocket_key()?;
        if let Err(err) = write!(
            stdin,
            "GET {} HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {}\r\nSec-WebSocket-Version: 13\r\n\r\n",
            parsed.path_and_query, parsed.host, key
        ) {
            return Err(finalize_failure(
                &mut child,
                stderr.take(),
                format!("failed to write Feishu WebSocket upgrade request: {err}"),
            ));
        }
        if let Err(err) = stdin.flush() {
            return Err(finalize_failure(
                &mut child,
                stderr.take(),
                format!("failed to flush Feishu WebSocket upgrade request: {err}"),
            ));
        }

        let mut status = String::new();
        if let Err(err) = stdout.read_line(&mut status) {
            return Err(finalize_failure(
                &mut child,
                stderr.take(),
                format!("failed to read Feishu WebSocket upgrade response: {err}"),
            ));
        }
        if !status.contains(" 101 ") {
            return Err(finalize_failure(
                &mut child,
                stderr.take(),
                format!("Feishu WebSocket upgrade failed: {}", status.trim()),
            ));
        }
        loop {
            let mut line = String::new();
            if let Err(err) = stdout.read_line(&mut line) {
                return Err(finalize_failure(
                    &mut child,
                    stderr.take(),
                    format!("failed to read Feishu WebSocket upgrade headers: {err}"),
                ));
            }
            if line == "\r\n" || line == "\n" || line.is_empty() {
                break;
            }
        }

        Ok(Self {
            child,
            stdin,
            stdout,
            stderr,
        })
    }

    /// Convert a post-connect failure into a [`ChannelError`], killing the
    /// openssl subprocess and appending whatever it last wrote to stderr.
    fn fail(&mut self, primary: String) -> ChannelError {
        finalize_failure(&mut self.child, self.stderr.take(), primary)
    }

    pub(super) fn read_frame(&mut self) -> Result<Vec<u8>, ChannelError> {
        loop {
            let mut header = [0_u8; 2];
            if let Err(err) = self.stdout.read_exact(&mut header) {
                return Err(self.fail(format!("Feishu WebSocket read header failed: {err}")));
            }
            let opcode = header[0] & 0x0f;
            let masked = header[1] & 0x80 != 0;
            let mut len = u64::from(header[1] & 0x7f);
            if len == 126 {
                let mut bytes = [0_u8; 2];
                if let Err(err) = self.stdout.read_exact(&mut bytes) {
                    return Err(self.fail(format!("Feishu WebSocket read length-16 failed: {err}")));
                }
                len = u64::from(u16::from_be_bytes(bytes));
            } else if len == 127 {
                let mut bytes = [0_u8; 8];
                if let Err(err) = self.stdout.read_exact(&mut bytes) {
                    return Err(self.fail(format!("Feishu WebSocket read length-64 failed: {err}")));
                }
                len = u64::from_be_bytes(bytes);
            }
            let mask = if masked {
                let mut mask = [0_u8; 4];
                if let Err(err) = self.stdout.read_exact(&mut mask) {
                    return Err(self.fail(format!("Feishu WebSocket read mask failed: {err}")));
                }
                Some(mask)
            } else {
                None
            };
            if len > 16 * 1024 * 1024 {
                return Err(self.fail("Feishu WebSocket frame is too large".to_owned()));
            }
            let mut payload = vec![0_u8; len as usize];
            if let Err(err) = self.stdout.read_exact(&mut payload) {
                return Err(self.fail(format!("Feishu WebSocket read payload failed: {err}")));
            }
            if let Some(mask) = mask {
                for (index, byte) in payload.iter_mut().enumerate() {
                    *byte ^= mask[index % 4];
                }
            }
            match opcode {
                0x2 => return Ok(payload),
                0x8 => {
                    return Err(self.fail("Feishu WebSocket closed by server".to_owned()));
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
        if let Err(err) = self
            .stdin
            .write_all(&frame)
            .and_then(|_| self.stdin.flush())
        {
            return Err(self.fail(format!("Feishu WebSocket write failed: {err}")));
        }
        Ok(())
    }
}

/// Kill the openssl subprocess, drain whatever it last wrote to stderr (capped
/// at [`STDERR_CAPTURE_LIMIT`] bytes), and fold that text into the returned
/// error so operators see the actual TLS / DNS / cert failure instead of a
/// bare upgrade message.
fn finalize_failure(
    child: &mut Child,
    stderr: Option<ChildStderr>,
    primary: String,
) -> ChannelError {
    let _ = child.kill();
    let _ = child.wait();
    let captured = drain_stderr(stderr);
    let message = if captured.is_empty() {
        primary
    } else {
        format!("{primary}\nopenssl stderr: {captured}")
    };
    ChannelError::Backend(Arc::from(message))
}

fn drain_stderr(handle: Option<ChildStderr>) -> String {
    let Some(handle) = handle else {
        return String::new();
    };
    let mut buf = Vec::new();
    let _ = handle.take(STDERR_CAPTURE_LIMIT).read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).trim().to_owned()
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

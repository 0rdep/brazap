use std::{
    fs::{File, OpenOptions},
    io::ErrorKind,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
};

use crate::{config::Config, mcp, model::Snapshot, service::Service};

const MAX_FRAME: u64 = 32 * 1024 * 1024;

fn lock_is_held(error: &std::io::Error) -> bool {
    error.kind() == ErrorKind::WouldBlock
        || (cfg!(windows) && matches!(error.raw_os_error(), Some(32 | 33)))
}

/// Keep the lease alive for the whole UI lifetime, independently of a headless service.
pub fn desktop_lease(config: &Config) -> Result<Option<File>> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(config.data_dir.join("desktop.lock"))?;
    match fs2::FileExt::try_lock_exclusive(&file) {
        Ok(()) => Ok(Some(file)),
        Err(error) if lock_is_held(&error) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// A second launch can race the first window's service/desktop registration.
pub async fn activate_existing(config: &Config, chat: Option<&str>) -> Result<Value> {
    tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            if let Ok(remote) = Remote::load(config)
                && let Ok(Ok(value)) = tokio::time::timeout(
                    Duration::from_secs(1), remote.request("brazap/activate", json!({"chat":chat})),
                ).await
                && value["accepted"].as_bool() == Some(true)
            {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }).await.context("Brazap is already running but its window is not responding yet. Try opening it again shortly.")
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Remote {
    address: std::net::SocketAddr,
    token: String,
}

pub struct Instance {
    pub remote: Remote,
    pub owned: Option<Arc<Service>>,
}

impl Remote {
    pub fn load(config: &Config) -> Result<Self> {
        let remote: Self = serde_json::from_slice(
            &std::fs::read(config.data_dir.join("instance.json")).context(
                "Open Brazap, or run `brazap --headless`, before connecting an MCP client",
            )?,
        )?;
        ensure!(
            remote.address.ip().is_loopback(),
            "Refusing a non-local service address"
        );
        Ok(remote)
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let mut stream = BufReader::new(
            TcpStream::connect(self.address)
                .await
                .context("The local Brazap service is not running")?,
        );
        let message = json!({"jsonrpc":"2.0","id":1,"method":method,"params":params});
        write_message(
            stream.get_mut(),
            &json!({"token":self.token,"message":message}),
        )
        .await?;
        let response = tokio::time::timeout(Duration::from_secs(120), read_message(&mut stream))
            .await??
            .context("Brazap disconnected")?;
        if let Some(error) = response.get("error") {
            bail!(
                "{}",
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Request failed")
            );
        }
        response.get("result").cloned().context("Missing result")
    }

    pub async fn snapshot(&self, chat: Option<&str>) -> Result<Snapshot> {
        Ok(serde_json::from_value(
            self.request("brazap/snapshot", json!({"chat":chat}))
                .await?,
        )?)
    }

    pub async fn asset(&self, request: crate::assets::AssetRequest) -> Result<Option<Vec<u8>>> {
        let asset: Option<crate::assets::Asset> = serde_json::from_value(
            self.request("brazap/asset", serde_json::to_value(request)?)
                .await?,
        )?;
        asset.map(|a| a.bytes()).transpose()
    }

    pub async fn call(&self, name: &str, args: Value) -> Result<Value> {
        self.request("brazap/call", json!({"name":name,"arguments":args}))
            .await
    }

    pub async fn stdio(&self) -> Result<()> {
        let mut stream = BufReader::new(TcpStream::connect(self.address).await?);
        let mut input = BufReader::new(tokio::io::stdin());
        let mut output = tokio::io::stdout();
        loop {
            let message = match read_message(&mut input).await {
                Ok(Some(message)) => message,
                Ok(None) => break,
                Err(error) if error.downcast_ref::<serde_json::Error>().is_some() => {
                    write_message(&mut output, &json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}})).await?;
                    continue;
                }
                Err(error) => return Err(error),
            };
            let needs_reply = message.get("id").is_some();
            write_message(
                stream.get_mut(),
                &json!({"token":self.token,"message":message}),
            )
            .await?;
            if needs_reply {
                let reply = read_message(&mut stream)
                    .await?
                    .context("Brazap disconnected")?;
                write_message(&mut output, &reply).await?;
            }
        }
        Ok(())
    }
}

pub async fn start_or_connect(config: Config) -> Result<Instance> {
    let lock_path = config.data_dir.join("service.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    match fs2::FileExt::try_lock_exclusive(&file) {
        Ok(()) => {
            let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
            let service = Service::open(&config).await?;
            let remote = Remote {
                address: listener.local_addr()?,
                token: uuid::Uuid::new_v4().to_string(),
            };
            let path = config.data_dir.join("instance.json");
            let mut options = OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            use std::io::Write;
            options
                .open(path)?
                .write_all(&serde_json::to_vec(&remote)?)?;
            tokio::spawn(service.clone().run());
            tokio::spawn(serve(listener, service.clone(), remote.token.clone(), file));
            Ok(Instance {
                remote,
                owned: Some(service),
            })
        }
        Err(error) if lock_is_held(&error) => {
            for _ in 0..50 {
                if let Ok(remote) = Remote::load(&config)
                    && remote.request("ping", json!({})).await.is_ok()
                {
                    return Ok(Instance {
                        remote,
                        owned: None,
                    });
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            bail!("Another Brazap instance owns this data directory but is not responding")
        }
        Err(error) => Err(error.into()),
    }
}

async fn serve(listener: TcpListener, service: Arc<Service>, token: String, _lock: File) {
    while let Ok((stream, _)) = listener.accept().await {
        let (service, token) = (service.clone(), token.clone());
        tokio::spawn(async move {
            if let Err(error) = connection(stream, service, token).await {
                log::debug!("Local client disconnected: {error}");
            }
        });
    }
}

async fn connection(stream: TcpStream, service: Arc<Service>, token: String) -> Result<()> {
    let mut stream = BufReader::new(stream);
    let mut session = mcp::Session::default();
    while let Some(envelope) = read_message(&mut stream).await? {
        ensure!(
            envelope.get("token").and_then(Value::as_str) == Some(token.as_str()),
            "Local client authentication failed"
        );
        let request = envelope
            .get("message")
            .cloned()
            .context("Missing request")?;
        if let Some(response) = mcp::handle(&service, request, &mut session).await {
            write_message(stream.get_mut(), &response).await?;
        }
    }
    Ok(())
}

async fn read_message<R: tokio::io::AsyncBufRead + Unpin>(reader: &mut R) -> Result<Option<Value>> {
    let mut line = String::new();
    let count = reader.take(MAX_FRAME + 1).read_line(&mut line).await?;
    if count == 0 {
        return Ok(None);
    }
    ensure!(
        count as u64 <= MAX_FRAME && line.ends_with('\n'),
        "Message exceeds the local protocol limit"
    );
    Ok(Some(serde_json::from_str(&line)?))
}

async fn write_message<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    value: &Value,
) -> Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}

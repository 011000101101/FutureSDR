use async_net::{TcpListener, TcpStream};
use futures::AsyncReadExt;
use futures::AsyncWriteExt;
use futuresdr::log::{debug, info, warn};
use std::thread::sleep;
use std::time::Duration;

use futuresdr::anyhow::{bail, Context, Result};
use futuresdr::macros::async_trait;
use futuresdr::runtime::Block;
use futuresdr::runtime::BlockMeta;
use futuresdr::runtime::BlockMetaBuilder;
use futuresdr::runtime::Kernel;
use futuresdr::runtime::MessageIo;
use futuresdr::runtime::MessageIoBuilder;
use futuresdr::runtime::StreamIo;
use futuresdr::runtime::StreamIoBuilder;
use futuresdr::runtime::WorkIo;

pub struct TcpSource {
    remote_socket: String,
    socket: Option<TcpStream>,
}

pub struct TcpSink {
    port: u32,
    socket: Option<TcpStream>,
}

impl TcpSource {
    pub fn new(remote_socket: String) -> Block {
        Block::new(
            BlockMetaBuilder::new("TcpSource").build(),
            StreamIoBuilder::new().add_output::<u8>("out").build(),
            MessageIoBuilder::new().build(),
            TcpSource {
                remote_socket,
                socket: None,
            },
        )
    }
}

impl TcpSink {
    pub fn new(port: u32) -> Block {
        Block::new(
            BlockMetaBuilder::new("TcpSink").build(),
            StreamIoBuilder::new().add_input::<u8>("in").build(),
            MessageIoBuilder::new().build(),
            TcpSink { port, socket: None },
        )
    }
}

#[doc(hidden)]
#[async_trait]
impl Kernel for TcpSource {
    async fn work(
        &mut self,
        io: &mut WorkIo,
        sio: &mut StreamIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
    ) -> Result<()> {
        let out = sio.output(0).slice::<u8>();
        if out.is_empty() {
            return Ok(());
        }

        match self
            .socket
            .as_mut()
            .context("no socket")?
            .read_exact(out)
            .await
        {
            Ok(_) => {
                debug!("tcp source read bytes {}", out.len());
                sio.output(0).produce(out.len());
            }
            Err(_) => {
                debug!("tcp source socket closed");
                io.finished = true;
            }
        }

        Ok(())
    }

    async fn init(
        &mut self,
        _sio: &mut StreamIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
    ) -> Result<()> {
        while self.socket.is_none() {
            if let Ok(socket) = TcpStream::connect(self.remote_socket.clone()).await {
                self.socket = Some(socket);
            } else {
                warn!(
                    "could not connect local TCP source to remote TCP sink yet, retrying in 5s..."
                );
                sleep(Duration::from_secs(5));
            }
        }
        info!(
            "connected local TCP source to remote tcp sink ({})",
            self.remote_socket
        );
        Ok(())
    }
}

#[doc(hidden)]
#[async_trait]
impl Kernel for TcpSink {
    async fn work(
        &mut self,
        io: &mut WorkIo,
        sio: &mut StreamIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
    ) -> Result<()> {
        let i = sio.input(0).slice::<u8>();

        match self
            .socket
            .as_mut()
            .context("no socket")?
            .write_all(i)
            .await
        {
            Ok(()) => {}
            Err(_) => bail!("tcp sink socket error"),
        }

        if sio.input(0).finished() {
            io.finished = true;
        }

        debug!("tcp sink wrote bytes {}", i.len());
        sio.input(0).consume(i.len());

        Ok(())
    }

    async fn init(
        &mut self,
        _sio: &mut StreamIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
    ) -> Result<()> {
        let mut listener = Some(TcpListener::bind(format!("0.0.0.0:{}", self.port)).await?);
        let (socket, _) = listener.as_mut().context("no listener")?.accept().await?;
        self.socket = Some(socket);
        info!("remote tcp exchanger accepted connection");
        Ok(())
    }
}

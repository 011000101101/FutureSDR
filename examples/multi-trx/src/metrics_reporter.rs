use futuresdr::anyhow::Result;
use futuresdr::async_io::block_on;
use futuresdr::async_net::UdpSocket;
use futuresdr::log::warn;
use futuresdr::macros::async_trait;
use futuresdr::macros::message_handler;
use futuresdr::runtime::Block;
use futuresdr::runtime::BlockMeta;
use futuresdr::runtime::BlockMetaBuilder;
use futuresdr::runtime::Kernel;
use futuresdr::runtime::MessageIo;
use futuresdr::runtime::MessageIoBuilder;
use futuresdr::runtime::Pmt;
use futuresdr::runtime::StreamIoBuilder;
use futuresdr::runtime::WorkIo;

use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::time::SystemTime;

static TUN_INTERFACE_HEADER_LEN: usize = 4;

pub struct MetricsReporter {
    socket_metrics: UdpSocket,
    local_ip: String,
}

impl MetricsReporter {
    pub fn new(remote_socket: String, local_ip: String) -> Block {
        let socket_metrics = block_on(UdpSocket::bind("0.0.0.0:0")).unwrap();
        block_on(socket_metrics.connect(remote_socket)).unwrap();
        Block::new(
            BlockMetaBuilder::new("MetricsReporter").build(),
            StreamIoBuilder::new().build(),
            MessageIoBuilder::new()
                .add_input("rx_in", Self::message_in_rx)
                .add_input("rx_wifi_in", Self::message_in_rx_wifi)
                .add_input("tx_in", Self::message_in_tx)
                .build(),
            MetricsReporter {
                socket_metrics: socket_metrics,
                local_ip: local_ip,
            },
        )
    }

    #[message_handler]
    async fn message_in_rx(
        &mut self,
        _io: &mut WorkIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
        p: Pmt,
    ) -> Result<Pmt> {
        self.message_in(_mio, _meta, p, "rx").await
    }

    #[message_handler]
    async fn message_in_rx_wifi(
        &mut self,
        _io: &mut WorkIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
        p: Pmt,
    ) -> Result<Pmt> {
        if let Pmt::Blob(buf) = p {
            self.message_in(_mio, _meta, Pmt::Blob(buf[24..].to_vec()), "rx")
                .await
        } else {
            warn!("pmt to tx was not a blob");
            Ok(Pmt::Null)
        }
    }

    #[message_handler]
    async fn message_in_tx(
        &mut self,
        _io: &mut WorkIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
        p: Pmt,
    ) -> Result<Pmt> {
        self.message_in(_mio, _meta, p, "tx").await
    }

    async fn message_in(
        &mut self,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
        p: Pmt,
        direction: &str,
    ) -> Result<Pmt> {
        if let Pmt::Blob(buf) = p {
            let dscp_val = buf[TUN_INTERFACE_HEADER_LEN + 1];
            let next_protocol = buf[TUN_INTERFACE_HEADER_LEN + 9];
            let timestamp = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs_f64();
            let mut s = DefaultHasher::new();
            buf.hash(&mut s);
            let hash = s.finish();
            if let Err(_) = self
                .socket_metrics
                .send(
                    format!(
                        "{},{},{},{},{},{},{}",
                        self.local_ip,
                        direction,
                        buf.len(),
                        dscp_val,
                        next_protocol,
                        hash,
                        timestamp
                    )
                    .as_bytes(),
                )
                .await
            {
                // if let Err(_) = self.socket_metrics.send(format!("{},{}", self.local_ip, direction).as_bytes()).await {
                warn!("could not send metric update.");
            }
        } else {
            warn!("pmt to tx was not a blob");
        }
        Ok(Pmt::Null)
    }
}

#[async_trait]
impl Kernel for MetricsReporter {}

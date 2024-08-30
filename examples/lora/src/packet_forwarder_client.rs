use std::net::SocketAddr;
use std::str::FromStr;

use semtech_udp::{Bandwidth, CodingRate, DataRate, MacAddress, SpreadingFactor};
use semtech_udp::client_runtime::{Event, UdpRuntime};
use semtech_udp::push_data::{CRC, Packet, RxPk, RxPkV2};
use tokio::runtime::Runtime;
use triggered::Trigger;

use futuresdr::anyhow::Result;
use futuresdr::futures::channel::mpsc;
use futuresdr::futures::channel::mpsc::Sender;
use futuresdr::futures::SinkExt;
use futuresdr::futures_lite::StreamExt;
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

/// Forward messages.
pub struct PacketForwarderClient {
    mac_addr: MacAddress,
    shutdown_trigger: Trigger,
    uplink_sender: Sender<Packet>,
    #[allow(dead_code)]
    udp_client_runtime: Runtime,
}

impl PacketForwarderClient {
    pub fn new(mac_addr: &str, server_addr: &str) -> Block {
        let mac_address = MacAddress::from_str(mac_addr).unwrap();
        let (to_forwarder_sender, mut to_forwarder_receiver) = mpsc::channel::<Packet>(1);
        let host = SocketAddr::from_str(server_addr).unwrap();
        let server_addr = server_addr.to_owned();

        let (shutdown_trigger, shutdown_signal) = triggered::trigger();
        let shutdown_trigger_tmp = shutdown_trigger.clone();

        let rt_tokio = Runtime::new().unwrap();
        let handle = rt_tokio.handle();

        rt_tokio.block_on(async move {
            let (uplink_sender, mut downlink_request_receiver, udp_runtime) =
                UdpRuntime::new(mac_address, host).await.unwrap();
            println!("Connecting to server {}", server_addr);
            let udp_runtime_task = handle.spawn(udp_runtime.run(shutdown_signal));

            // send received frames
            handle.spawn(async move {
                while let Some(received_frame) = to_forwarder_receiver.next().await {
                    uplink_sender.send(received_frame).await.unwrap();
                }
            });

            // report problems with the server connection and handle downlink request acks
            handle.spawn(async move {
                while let Some(event) = downlink_request_receiver.recv().await {
                    match event {
                        Event::LostConnection => {
                            println!("Lost connection to GWMP client");
                        }
                        Event::Reconnected => {
                            println!("Reconnected to GWMP client");
                        }
                        Event::DownlinkRequest(downlink_request) => {
                            downlink_request.ack().await.unwrap()
                        }
                        Event::UnableToParseUdpFrame(parse_error, _buffer) => {
                            println!("Error parsing UDP frame {parse_error}")
                        }
                    }
                }
                shutdown_trigger.trigger();
                if let Err(e) = udp_runtime_task.await {
                    println!("UdpRunTime return error {e}");
                }
            });
        });

        Block::new(
            BlockMetaBuilder::new("MessageCopy").build(),
            StreamIoBuilder::new().build(),
            MessageIoBuilder::new()
                .add_input("in", Self::handler)
                .build(),
            PacketForwarderClient {
                mac_addr: mac_address,
                shutdown_trigger: shutdown_trigger_tmp,
                uplink_sender: to_forwarder_sender,
                udp_client_runtime: rt_tokio,
            },
        )
    }

    #[message_handler]
    async fn handler(
        &mut self,
        io: &mut WorkIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
        p: Pmt,
    ) -> Result<Pmt> {
        match p {
            Pmt::Finished => {
                self.shutdown_trigger.trigger();
                io.finished = true;
            }
            Pmt::MapStrPmt(m) => {
                if let Pmt::Blob(payload) = m.get("payload").unwrap() {
                    let codr: CodingRate = match m.get("codr").unwrap() {
                        Pmt::U32(1) => CodingRate::_4_5,
                        Pmt::U32(2) => CodingRate::_4_6,
                        Pmt::U32(3) => CodingRate::_4_7,
                        Pmt::U32(4) => CodingRate::_4_8,
                        _ => panic!("invalid Coding Rate in received msg: {:?}", m.get("codr")),
                    };
                    let sf: SpreadingFactor = match m.get("sf").unwrap() {
                        Pmt::U32(5) => SpreadingFactor::SF5,
                        Pmt::U32(6) => SpreadingFactor::SF6,
                        Pmt::U32(7) => SpreadingFactor::SF7,
                        Pmt::U32(8) => SpreadingFactor::SF8,
                        Pmt::U32(9) => SpreadingFactor::SF9,
                        Pmt::U32(10) => SpreadingFactor::SF10,
                        Pmt::U32(11) => SpreadingFactor::SF11,
                        Pmt::U32(12) => SpreadingFactor::SF12,
                        _ => panic!(
                            "invalid Spreading Factor in received msg: {:?}",
                            m.get("sf")
                        ),
                    };
                    let bw: Bandwidth = match m.get("bw").unwrap() {
                        Pmt::U32(125) => Bandwidth::BW125,
                        Pmt::U32(250) => Bandwidth::BW250,
                        Pmt::U32(500) => Bandwidth::BW500,
                        _ => panic!("invalid Bandwidth in received msg: {:?}", m.get("bw")),
                    };
                    let freq: f64 = match m.get("freq").unwrap() {
                        Pmt::F64(f) => *f,
                        _ => panic!("invalid Frequency in received msg: {:?}", m.get("bw")),
                    };
                    let rxpk = RxPk::V2(RxPkV2 {
                        aesk: 0,
                        brd: 0,
                        codr,
                        data: payload.clone(),
                        datr: DataRate::new(sf, bw),
                        freq,
                        jver: 2,
                        modu: "LORA".to_owned(),
                        rsig: vec![], // TODO
                        size: payload.len() as u64,
                        stat: CRC::OK,
                        tmst: 0, // TODO
                        delayed: None,
                        tmms: None,
                        time: None,
                    });
                    let packet = semtech_udp::push_data::Packet::from_rxpk(self.mac_addr, rxpk);
                    self.uplink_sender.send(packet).await.unwrap();
                } else {
                    panic!(
                        "invalid payload in received msg. Expected Pmt::Blob, got {:?}",
                        m.get("payload")
                    )
                }
            }
            _ => panic!("invalid message type to msg in port. Expected Pmt::MapStrPmt, got {p}"),
        }
        Ok(Pmt::Ok)
    }
}

#[doc(hidden)]
#[async_trait]
impl Kernel for PacketForwarderClient {}

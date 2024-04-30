use std::collections::HashMap;
use std::time::Instant;
use std::net::{Ipv4Addr, UdpSocket};
use mavlink::common::*;
use mavlink::common::MavMessage::*;
//use mavlink::MavHeader;

use futuresdr::anyhow::Result;
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

static TUN_INTERFACE_HEADER_LEN: usize = 4;

pub fn global_to_relative(origin_lat: f64, origin_lng: f64, lat: f64, lng: f64) -> (f64, f64) {
    let rel_x = (lng - origin_lng) * 40_075_000.0 * origin_lat.to_radians().cos() / 360.0;
    let rel_y = (lat - origin_lat) * 111_320.0;
    (rel_x, rel_y)
}

pub struct IPDSCPRewriter {
    flow_priority_map: HashMap<u16, u8>,
    boot: Instant,
    armed: bool,
    ground_station_pos: (f64, f64, f64),
    last_position_timestamp: u32,
    remote: Option<UdpSocket>,
}

impl IPDSCPRewriter {
    pub fn new(
        new_flow_priority_map: HashMap<u16, u8>,
        position_update_receiver: Option<&str>,
        ground_station_pos: (f64, f64, f64)
    ) -> Block {

        let s = if let Some(update_receiver) = position_update_receiver {
            let s_tmp = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).unwrap();
            s_tmp.connect(update_receiver).unwrap();
            Some(s_tmp)
        } else {
            None
        };

        Block::new(
            BlockMetaBuilder::new("IPDSCPRewriter").build(),
            StreamIoBuilder::new().build(),
            MessageIoBuilder::new()
                .add_input("in", Self::message_in)
                .add_output("out")
                .build(),
            IPDSCPRewriter {
                flow_priority_map: new_flow_priority_map,
                boot: Instant::now(),
                armed: false,
                ground_station_pos,
                last_position_timestamp: 0,
                remote: s,
            },
        )
    }

    fn process_mavlink_msg(&mut self, msg: mavlink::common::MavMessage) {

        match msg {
            HEARTBEAT(data) => {
                if self.boot.elapsed().as_secs_f32() > 30.0 && data.system_status == MavState::MAV_STATE_ACTIVE {
                    self.armed = true;
                }
            }
            GLOBAL_POSITION_INT(data) if data.time_boot_ms > self.last_position_timestamp => {
                let lat = data.lat as f64 / 10_000_000.0;
                let lng = data.lon as f64 / 10_000_000.0;
                let alt = data.alt as f32 / 1000.0;

                println!("Got Position: ({}, {}, {})", lat, lng, alt);

                let (mut rel_x, mut rel_y) = global_to_relative(self.ground_station_pos.0, self.ground_station_pos.1, lat, lng);
                let mut rel_z = alt - self.ground_station_pos.2 as f32;
                if !self.armed {
                    rel_x = 0.0;
                    rel_y = 0.0;
                    rel_z = 0.0;
                }
                self.last_position_timestamp = data.time_boot_ms;

                let mut bin_pos = [0u8; 24];

                bin_pos[0..4].copy_from_slice(&(rel_x as f32).to_be_bytes());
                bin_pos[4..8].copy_from_slice(&(rel_y as f32).to_be_bytes());
                bin_pos[8..12].copy_from_slice(&(rel_z as f32).to_be_bytes());
                bin_pos[12..16].copy_from_slice(&0.0_f32.to_be_bytes());
                bin_pos[16..20].copy_from_slice(&0.0_f32.to_be_bytes());
                bin_pos[20..24].copy_from_slice(&0.0_f32.to_be_bytes());

                println!("Updating Position: ({}, {}, {})", rel_x, rel_y, rel_z);

                if let Some(r) = &self.remote {
                    if r.send(&bin_pos).is_err()
                    {
                        warn!("updating via UDP failed");
                    }
                }
            },
            _ => {}
        }
    }

    fn process_mavlink(&mut self, bytes: &[u8]) {
        if bytes.len() > 0 {
            let stx = bytes[0];
            let mut cursor = std::io::Cursor::new(bytes);

            use mavlink::common::*;

            if stx == 0xfd {
                if let Ok((_header, msg)) = mavlink::read_v2_msg::<MavMessage, _>(&mut cursor) {
                    self.process_mavlink_msg(msg);
                }
            } else {  // TODO
                if let Ok((_header, msg)) = mavlink::read_v1_msg::<MavMessage, _>(&mut cursor) {
                    self.process_mavlink_msg(msg);
                }
            }
        } else {
            warn!("received zero-length mavlink message, dropping.");
        }
    }

    #[message_handler]
    async fn message_in(
        &mut self,
        _io: &mut WorkIo,
        mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
        p: Pmt,
    ) -> Result<Pmt> {
        if let Pmt::Blob(mut buf) = p {
            let next_protocol = buf[TUN_INTERFACE_HEADER_LEN + 9] as usize;
            if next_protocol == 6_usize || next_protocol == 17_usize {
                let ip_header_length =
                    ((buf[TUN_INTERFACE_HEADER_LEN] & 0b00001111) as usize * 4_usize) as usize;
                // let src_port = ((buf[4 + ip_header_length] as u16) << 8) | (buf[4 + ip_header_length + 1] as u16);
                let dst_port = ((buf[TUN_INTERFACE_HEADER_LEN + ip_header_length + 2] as u16) << 8)
                    | (buf[TUN_INTERFACE_HEADER_LEN + ip_header_length + 3] as u16);
                // println!("{}", format!("src: {}, dst: {}", src_port, dst_port));
                if self.remote.is_some() && dst_port == 14550 {  // TODO filter mavlink messages
                    // println!("trying to intercept position update on port {dst_port}");
                    self.process_mavlink(&buf[(TUN_INTERFACE_HEADER_LEN + ip_header_length + 8)..]);
                }
                if let Some(new_dscp_val) = self.flow_priority_map.get(&dst_port) {
                    // println!("Replacing old dscp {:#8b} with new value {:#8b}", buf[5], new_dscp_val);
                    buf[TUN_INTERFACE_HEADER_LEN + 1] = *new_dscp_val;
                    // if we change the header, we need to recompute and update the checksum, else the packet will be discarded at the receiver
                    let mut new_checksum = 0_u16;
                    for i in 0..5 {
                        let (new_checksum_tmp, carry) = new_checksum.overflowing_add(
                            ((buf[TUN_INTERFACE_HEADER_LEN + 2 * i] as u16) << 8)
                                + (buf[TUN_INTERFACE_HEADER_LEN + 2 * i + 1] as u16),
                        );
                        new_checksum = if carry {
                            new_checksum_tmp + 1
                        } else {
                            new_checksum_tmp
                        };
                    }
                    for i in 6..(ip_header_length / 2) {
                        let (new_checksum_tmp, carry) = new_checksum.overflowing_add(
                            ((buf[TUN_INTERFACE_HEADER_LEN + 2 * i] as u16) << 8)
                                + (buf[TUN_INTERFACE_HEADER_LEN + 2 * i + 1] as u16),
                        );
                        new_checksum = if carry {
                            new_checksum_tmp + 1
                        } else {
                            new_checksum_tmp
                        };
                    }
                    new_checksum = !new_checksum;
                    buf[TUN_INTERFACE_HEADER_LEN + 10] = (new_checksum >> 8) as u8;
                    buf[TUN_INTERFACE_HEADER_LEN + 11] = (new_checksum & 0b0000000011111111) as u8;
                }
            }
            mio.output_mut(0).post(Pmt::Blob(buf)).await;
        } else {
            warn!("pmt to tx was not a blob");
        }
        Ok(Pmt::Null)
    }
}

#[async_trait]
impl Kernel for IPDSCPRewriter {}

use std::collections::HashMap;

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

pub struct IPDSCPRewriter {
    flow_priority_map: HashMap<u16, u8>,
}

impl IPDSCPRewriter {
    pub fn new(new_flow_priority_map: HashMap<u16, u8>) -> Block {
        Block::new(
            BlockMetaBuilder::new("IPDSCPRewriter").build(),
            StreamIoBuilder::new().build(),
            MessageIoBuilder::new()
                .add_input("in", Self::message_in)
                .add_output("out")
                .build(),
            IPDSCPRewriter {
                flow_priority_map: new_flow_priority_map,
            },
        )
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

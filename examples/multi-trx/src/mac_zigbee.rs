use util_blocks::{BoundedDiscretePriorityQueue, PRIORITY_VALUES};

// use std::future::Future;
// use std::pin::Pin;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

use futuresdr::anyhow::Result;
use futuresdr::macros::async_trait;
// use futuresdr::futures::FutureExt;
use futuresdr::log::{debug, warn};
use futuresdr::macros::message_handler;
use futuresdr::runtime::Block;
use futuresdr::runtime::BlockMeta;
use futuresdr::runtime::BlockMetaBuilder;
use futuresdr::runtime::Kernel;
use futuresdr::runtime::MessageIo;
use futuresdr::runtime::MessageIoBuilder;
use futuresdr::runtime::Pmt;
use futuresdr::runtime::StreamIo;
use futuresdr::runtime::StreamIoBuilder;
use futuresdr::runtime::Tag;
use futuresdr::runtime::WorkIo;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
extern "C" {
    fn rxed_frame(s: Vec<u8>);
}

const MAX_FRAMES: usize = 128;
const MAX_FRAME_SIZE: usize = 256;

fn get_dscp_priority(data: &Vec<u8>) -> u8 {
    let dscp_index = 4 + 1; // 4 bytes added by TUN adapter + offset in IP header
    data[dscp_index]
}

pub struct Mac {
    tx_frames: BoundedDiscretePriorityQueue<'static, Vec<u8>, u8>,
    current_frame: [u8; MAX_FRAME_SIZE],
    current_index: usize,
    current_len: usize,
    n_received: u64,
    n_sent: u64,
}

impl Mac {
    pub fn new() -> Block {
        let mut b = [0; MAX_FRAME_SIZE];
        b[0] = 0x0;
        b[1] = 0x0;
        b[2] = 0x0;
        b[3] = 0xa7;
        b[4] = 0x0; // len

        Block::new(
            BlockMetaBuilder::new("Mac").build(),
            StreamIoBuilder::new().add_output::<u8>("out").build(),
            MessageIoBuilder::new()
                .add_input("rx", Self::received)
                .add_input("tx", Self::transmit)
                .add_input("stats", Self::stats)
                .add_input("flush_queue", Self::flush_queue)
                .add_output("rxed")
                .add_output("rftap")
                .build(),
            Mac {
                tx_frames: BoundedDiscretePriorityQueue::new(MAX_FRAMES, &PRIORITY_VALUES),
                current_frame: b,
                current_index: 0,
                current_len: 0,
                n_received: 0,
                n_sent: 0,
            },
        )
    }

    #[message_handler]
    pub fn flush_queue<'a>(
        &'a mut self,
        _io: &'a mut WorkIo,
        _mio: &'a mut MessageIo<Self>,
        _meta: &'a mut BlockMeta,
        _p: Pmt,
    ) -> Result<Pmt> {
        self.tx_frames.flush();
        Ok(Pmt::Null)
    }

    fn calc_crc(data: &[u8]) -> u16 {
        let mut crc: u16 = 0;

        for b in data.iter() {
            for k in 0..8 {
                let bit = if b & (1 << k) != 0 {
                    1 ^ (crc & 1)
                } else {
                    crc & 1
                };
                crc >>= 1;
                if bit != 0 {
                    crc ^= 1 << 15;
                    crc ^= 1 << 10;
                    crc ^= 1 << 3;
                }
            }
        }
        crc
    }

    fn check_crc(data: &[u8]) -> bool {
        Self::calc_crc(data) == 0
    }

    #[message_handler]
    fn received<'a>(
        &'a mut self,
        _io: &'a mut WorkIo,
        mio: &'a mut MessageIo<Self>,
        _meta: &'a mut BlockMeta,
        p: Pmt,
    ) -> Result<Pmt> {
        match p {
            Pmt::Blob(data) => {
                if Self::check_crc(&data) && data.len() > 2 {
                    debug!("received frame, crc correct, payload length {}", data.len());
                    #[cfg(target_arch = "wasm32")]
                    rxed_frame(data.clone());

                    let mut rftap = vec![0; data.len() + 12];
                    rftap[0..4].copy_from_slice("RFta".as_bytes());
                    rftap[4..6].copy_from_slice(&3u16.to_le_bytes());
                    rftap[6..8].copy_from_slice(&1u16.to_le_bytes());
                    rftap[8..12].copy_from_slice(&195u32.to_le_bytes());
                    rftap[12..].copy_from_slice(&data);
                    mio.output_mut(1).post(Pmt::Blob(rftap)).await;

                    self.n_received += 1;
                    let s = String::from_iter(
                        data.iter()
                            .map(|x| char::from(*x))
                            .map(|x| if x.is_ascii() { x } else { '.' })
                            .map(|x| {
                                if ['\x0b', '\x0c', '\n', '\t', '\r'].contains(&x) {
                                    '.'
                                } else {
                                    x
                                }
                            }),
                    );
                    debug!("{}", s);
                    mio.output_mut(0).post(Pmt::Blob(data)).await;
                } else {
                    debug!("received frame, crc wrong");
                }
            }
            _ => {
                warn!("ZigBee Mac: received wrong PMT type in RX callback (expected Pmt::Blob)");
            }
        }
        Ok(Pmt::Null)
    }

    #[message_handler]
    fn transmit<'a>(
        &'a mut self,
        _io: &'a mut WorkIo,
        _mio: &'a mut MessageIo<Self>,
        _meta: &'a mut BlockMeta,
        p: Pmt,
    ) -> Result<Pmt> {
        match p {
            Pmt::Blob(data) => {
                // 2 crc
                if data.len() > MAX_FRAME_SIZE - 5 {
                    warn!(
                        "ZigBee Mac: TX frame too large ({}, max {}). Dropping.",
                        data.len(),
                        MAX_FRAME_SIZE - 5
                    );
                } else {
                    let priority = get_dscp_priority(&data);
                    self.tx_frames.push_back(data, priority);
                }
            }
            _ => {
                warn!("ZigBee Mac: received wrong PMT type in TX callback (expected Pmt::Blob)");
            }
        }
        Ok(Pmt::Null)
    }

    #[message_handler]
    fn stats<'a>(
        &'a mut self,
        _io: &'a mut WorkIo,
        _mio: &'a mut MessageIo<Self>,
        _meta: &'a mut BlockMeta,
        _p: Pmt,
    ) -> Result<Pmt> {
        Ok(Pmt::VecU64(vec![self.n_sent, self.n_received]))
    }
}

#[async_trait]
impl Kernel for Mac {
    async fn work(
        &mut self,
        _io: &mut WorkIo,
        sio: &mut StreamIo,
        _m: &mut MessageIo<Self>,
        _b: &mut BlockMeta,
    ) -> Result<()> {
        loop {
            let out = sio.output(0).slice::<u8>();
            if out.is_empty() {
                break;
            }

            if self.current_len == 0 {
                if let Some(v) = self.tx_frames.pop_front() {
                    self.current_frame[4] = (v.len() + 2) as u8;
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            v.as_ptr(),
                            self.current_frame.as_mut_ptr().add(5),
                            v.len(),
                        );
                    }

                    let crc = Self::calc_crc(&self.current_frame[5..5 + v.len()]);
                    self.current_frame[5 + v.len()] = crc.to_le_bytes()[0];
                    self.current_frame[6 + v.len()] = crc.to_le_bytes()[1];

                    // 4 preamble + 1 len + 2 crc
                    self.current_len = v.len() + 7;
                    self.current_index = 0;
                    sio.output(0).add_tag(0, Tag::Id(self.current_len as u64));
                    debug!("sending frame, len {}", self.current_len);
                    self.n_sent += 1;
                    debug!("{:?}", &self.current_frame[0..self.current_len]);
                } else {
                    break;
                }
            } else {
                let n = std::cmp::min(out.len(), self.current_len - self.current_index);
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        self.current_frame.as_ptr().add(self.current_index),
                        out.as_mut_ptr(),
                        n,
                    );
                }

                sio.output(0).produce(n);
                self.current_index += n;

                if self.current_index == self.current_len {
                    self.current_len = 0;
                }
            }
        }

        Ok(())
    }
}

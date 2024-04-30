use futuresdr::anyhow::Result;
use futuresdr::macros::async_trait;

// use futuresdr::futures::FutureExt;

use futuresdr::log::warn;
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

use util_blocks::{BoundedDiscretePriorityQueue, PRIORITY_VALUES};

use lora::utilities::*;

pub struct Whitening {
    tx_frames: BoundedDiscretePriorityQueue<'static, Vec<u8>, u8>,
    quickstart_counter: usize,
}

const MAX_FRAMES: usize = 4;
// const MAX_FRAME_SIZE: usize = 256;

fn get_dscp_priority(data: &Vec<u8>) -> u8 {
    let dscp_index = 4 + 1; // 4 bytes added by TUN adapter + offset in IP header
    data[dscp_index]
}

impl Whitening {
    pub fn new() -> Block {
        Block::new(
            BlockMetaBuilder::new("Whitening").build(),
            StreamIoBuilder::new()
                // .add_input::<u8>("in")
                .add_output::<u8>("out")
                .build(),
            MessageIoBuilder::new()
                .add_input("tx", Self::msg_handler)
                .add_input("flush_queue", Self::flush_queue)
                .add_output("flush_propagate")
                .build(),
            Whitening {
                tx_frames: BoundedDiscretePriorityQueue::new(MAX_FRAMES, &PRIORITY_VALUES),
                quickstart_counter: 0,
            },
        )
    }

    #[message_handler]
    pub fn flush_queue<'a>(
        &'a mut self,
        _io: &'a mut WorkIo,
        mio: &'a mut MessageIo<Self>,
        _meta: &'a mut BlockMeta,
        _p: Pmt,
    ) -> Result<Pmt> {
        self.tx_frames.flush();
        self.quickstart_counter = 0;
        let flush_out_port_id = mio.output_name_to_id("flush_propagate").unwrap();
        mio.output_mut(flush_out_port_id).post(Pmt::Null).await;
        Ok(Pmt::Null)
    }

    #[message_handler]
    fn msg_handler(
        &mut self,
        io: &mut WorkIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
        p: Pmt,
    ) -> Result<Pmt> {
        if let Pmt::Blob(data) = p {
            {
                if data.len() > 255 {
                    println! {"LoRa: got large frame of {} samples.", data.len()};
                }
                let priority = get_dscp_priority(&data);
                if priority == 0 {
                    // if self.quickstart_counter < 1000000 {
                        // println! {"LoRa: dropping low-priority message."};
                        // self.quickstart_counter += 1;
                        return Ok(Pmt::Null);
                    // }
                    // TODO fake dropping non-priority for demo to get rid of long "transition time" due to large buffer count
                }
                // println! {"LoRa: queueing message (priority {}).", priority};
                self.tx_frames.push_back(data, priority);
                io.call_again = true;
            }
        } else {
            warn!("msg_handler pmt was not a String");
        }
        Ok(Pmt::Null)
    }
}

#[async_trait]
impl Kernel for Whitening {
    async fn work(
        &mut self,
        _io: &mut WorkIo,
        sio: &mut StreamIo,
        _m: &mut MessageIo<Self>,
        _b: &mut BlockMeta,
    ) -> Result<()> {
        // let input = sio.input(0).slice::<u8>();
        let out = sio.output(0).slice::<u8>();
        let out_capacity = out.len();
        let condition = move |data: &Vec<u8>| -> bool { data.len() * 2 <= out_capacity };
        if let Some(payload) = self.tx_frames.pop_front_with_condition(Box::new(condition)) {
            // println!("payload len: {}", payload.len());
            if payload.len() <= 256 {
                sio.output(0).add_tag(
                    0,
                    Tag::NamedAny(
                        "frame_len".to_string(),
                        Box::new(Pmt::Usize(2 * payload.len())),
                    ),
                );
                sio.output(0).add_tag(
                    0,
                    Tag::NamedAny(
                        "payload_str".to_string(),
                        // Box::new(Pmt::Blob(payload.clone())),
                        Box::new(Pmt::Blob(payload.clone())),
                    ),
                );
                for i in 0..payload.len() {
                    out[2 * i] = (payload[i] ^ WHITENING_SEQ[i]) & 0x0F;
                    out[2 * i + 1] = (payload[i] ^ WHITENING_SEQ[i]) >> 4;
                }
                sio.output(0).produce(2 * payload.len());
            } else {
                println!("DROPPING large packet: {} bytes", payload.len());
            }
        }
        Ok(())
    }
}

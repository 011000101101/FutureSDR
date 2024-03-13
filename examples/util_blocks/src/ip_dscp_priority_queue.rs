use crate::{BoundedDiscretePriorityQueue, PRIORITY_VALUES};

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
// use futuresdr::runtime::Tag;
use futuresdr::runtime::WorkIo;


const MAX_FRAMES: usize = 128;
// const MAX_FRAME_SIZE: usize = 256;

fn get_dscp_priority(data: &Vec<u8>) -> u8 {
    let dscp_index = 4 + 1; // 4 bytes added by TUN adapter + offset in IP header
    data[dscp_index]
}

pub struct IpDscpPriorityQueue {
    tx_frames: BoundedDiscretePriorityQueue<'static, Vec<u8>, u8>,
}

impl IpDscpPriorityQueue {
    pub fn new() -> Block {
        Block::new(
            BlockMetaBuilder::new("IpDscpPriorityQueue").build(),
            StreamIoBuilder::new().add_output::<u8>("out").build(),
            MessageIoBuilder::new()
                .add_input("tx", Self::transmit)
                .add_input("flush_queue", Self::flush_queue)
                .build(),
            IpDscpPriorityQueue {
                tx_frames: BoundedDiscretePriorityQueue::new(MAX_FRAMES, &PRIORITY_VALUES),
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

    #[message_handler]
    fn transmit<'a>(
        &'a mut self,
        io: &'a mut WorkIo,
        _mio: &'a mut MessageIo<Self>,
        _meta: &'a mut BlockMeta,
        p: Pmt,
    ) -> Result<Pmt> {
        match p {
            Pmt::Blob(data) => {
                let priority = get_dscp_priority(&data);
                self.tx_frames.push_back(data, priority);
                io.call_again = true;
            }
            _ => {
                warn!("IpDscpPriorityQueue: received wrong PMT type in TX callback (expected Pmt::Blob)");
            }
        }
        Ok(Pmt::Null)
    }
}

#[async_trait]
impl Kernel for IpDscpPriorityQueue {
    async fn work(
        &mut self,
        _io: &mut WorkIo,
        sio: &mut StreamIo,
        _m: &mut MessageIo<Self>,
        _b: &mut BlockMeta,
    ) -> Result<()> {
        let out = sio.output(0).slice::<u8>();

        // if we have something to send
        if let Some(v) = self.tx_frames.pop_front() {
            // and there is space in the output buffer
            if out.len() >= v.len() {
                // send it
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        v.as_ptr(),
                        out.as_mut_ptr(),
                        v.len(),
                    );
                }
                sio.output(0).produce(v.len());
            }
            // otherwise the priority queue will handle dropping old packets when inserting while queue is full
        }

        Ok(())
    }
}

use futuresdr::anyhow::Result;
use futuresdr::macros::async_trait;

use std::collections::VecDeque;

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

use crate::utilities::*;

pub struct Whitening {
    m_is_hex: bool, // indicate that the payload is given by a string of hex values
    // m_payload: Vec<u8>, // store the payload bytes
    payload_str: VecDeque<String>, // payload as a string
                                   // m_use_length_tag: bool, // wheter to use the length tag to separate frames or the separator character
                                   // m_input_byte_cnt: usize, // number of bytes from the input already processed
                                   // m_tag_offset: usize,
}

impl Whitening {
    pub fn new(is_hex: bool, use_length_tag: bool) -> Block {
        Block::new(
            BlockMetaBuilder::new("Whitening").build(),
            StreamIoBuilder::new()
                // .add_input::<u8>("in")
                .add_output::<u8>("out")
                .build(),
            MessageIoBuilder::new()
                .add_input("msg", Self::msg_handler)
                .build(),
            Whitening {
                // m_use_length_tag: use_length_tag,
                m_is_hex: if use_length_tag { false } else { is_hex }, // cant use length tag if input is given as a string of hex values
                // m_tag_offset: 1,
                // m_payload: vec![], // implicit
                payload_str: VecDeque::new(),
                // m_input_byte_cnt: 0,
            },
        )
    }

    #[message_handler]
    fn msg_handler(
        &mut self,
        _io: &mut WorkIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
        p: Pmt,
    ) -> Result<Pmt> {
        if let Pmt::String(payload) = p {
            {
                self.payload_str.push_back(payload);
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
        if self.payload_str.len() >= 100 && self.payload_str.len() % 100 == 0 {
            warn!("Whitening: frames in waiting list. Transmitter has issue to keep up at that transmission frequency.");
        }
        if !self.payload_str.is_empty()
            && out.len()
                >= if self.m_is_hex { 1 } else { 2 } * self.payload_str.front().unwrap().len()
        {
            let mut payload = self.payload_str.pop_front().unwrap();
            if self.m_is_hex {
                let len = payload.len();
                let mut new_string: Vec<char> = vec![];
                for i in (0..len).step_by(2) {
                    let byte = u8::from_str_radix(&payload[i..i + 2], 16).unwrap();
                    new_string.push(byte as char);
                }
                payload = new_string.into_iter().collect();
            }
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
                    Box::new(Pmt::Blob(payload.as_bytes().iter().cloned().collect())),
                ),
            );
            for i in 0..payload.len() {
                out[2 * i] = (payload.as_bytes()[i] ^ WHITENING_SEQ[i]) & 0x0F;
                out[2 * i + 1] = (payload.as_bytes()[i] ^ WHITENING_SEQ[i]) >> 4;
            }
            sio.output(0).produce(2 * payload.len());
        }
        Ok(())
    }
}

use futuresdr::anyhow::Result;
use futuresdr::macros::{async_trait, message_handler};
use std::cmp::min;

// use futuresdr::futures::FutureExt;

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

pub struct Header {
    m_impl_head: bool,           // indicate if the header is implicit
    m_has_crc: bool,             // indicate the presence of a payload crc
    m_cr: u8,                    // Transmission coding rate
    m_payload_len: usize,        // Payload length
    m_cnt_nibbles: usize,        // count the processes nibbles in a frame
    m_cnt_header_nibbles: usize, // count the number of explicit header nibbles output
    m_header: [u8; 5],           // contain the header to prepend
    m_tag_payload_len: usize,
    m_tag_payload_str: Vec<u8>,
    flush: bool,
}

impl Header {
    pub fn new(impl_head: bool, has_crc: bool, cr: u8) -> Block {
        Block::new(
            BlockMetaBuilder::new("Header").build(),
            StreamIoBuilder::new()
                .add_input::<u8>("in")
                .add_output::<u8>("out")
                .build(),
            MessageIoBuilder::new()
                .add_input("flush_queue", Self::flush_queue)
                .add_output("flush_propagate")
                .build(),
            Header {
                m_cr: cr,
                m_has_crc: has_crc,
                m_impl_head: impl_head,
                m_header: [0; 5],
                m_tag_payload_len: 0,
                m_tag_payload_str: vec![],
                m_cnt_header_nibbles: 0,
                m_payload_len: 0, // implicit
                m_cnt_nibbles: 0,
                flush: false,
            },
        )
    }

    #[message_handler]
    pub fn flush_queue<'a>(
        &'a mut self,
        io: &'a mut WorkIo,
        mio: &'a mut MessageIo<Self>,
        _meta: &'a mut BlockMeta,
        _p: Pmt,
    ) -> Result<Pmt> {
        self.flush = true;
        let flush_out_port_id = mio.output_name_to_id("flush_propagate").unwrap();
        mio.output_mut(flush_out_port_id).post(Pmt::Null).await;
        io.call_again = true;
        Ok(Pmt::Null)
    }

    // fn set_cr(&mut self, cr: u8) {
    //     self.m_cr = cr;
    // }
    //
    // fn get_cr(&self) -> u8 {
    //     return self.m_cr;
    // }
}

#[async_trait]
impl Kernel for Header {
    async fn work(
        &mut self,
        io: &mut WorkIo,
        sio: &mut StreamIo,
        _m: &mut MessageIo<Self>,
        _b: &mut BlockMeta,
    ) -> Result<()> {
        let input = sio.input(0).slice::<u8>();

        if self.flush {
            let count = input.len();
            sio.input(0).consume(count);
            self.m_header = [0; 5];
            self.m_tag_payload_len = 0;
            self.m_tag_payload_str = vec![];
            self.m_cnt_header_nibbles = 0;
            self.m_payload_len = 0;
            self.m_cnt_nibbles = 0;
            self.flush = false;
            io.call_again = true;
            return Ok(());
        }

        let out = sio.output(0).slice::<u8>();
        let mut nitems_to_process = min(input.len(), out.len());
        let mut out_offset: usize = 0;

        let tags: Vec<(usize, usize)> =
            get_tags_in_window::<usize>(sio.input(0).tags(), input.len(), "frame_len");
        // info! {"AddCrc: {:?}", tags};
        if !tags.is_empty() {
            if tags[0].0 != 0 {
                nitems_to_process = min(tags[0].0, out.len());
            } else {
                if tags.len() >= 2 {
                    nitems_to_process = min(tags[1].0, out.len());
                }

                self.m_payload_len = tags[0].1 / 2;
                //pass tags downstream
                self.m_tag_payload_len =
                    self.m_payload_len * 2 + if self.m_impl_head { 0 } else { 5 }; // 5 being the explicit header length

                let mut tags_tmp: Vec<(usize, Vec<u8>)> =
                    get_tags_in_window::<Vec<u8>>(sio.input(0).tags(), 1, "payload_str");
                // info! {"Header: {:?}", tags_tmp};
                self.m_cnt_nibbles = 0;
                assert!(tags_tmp.len() == 1);
                self.m_tag_payload_str = tags_tmp.pop().unwrap().1;
            }
        }
        if nitems_to_process > 0 {
            if self.m_cnt_nibbles == 0 && !self.m_impl_head {
                if self.m_cnt_header_nibbles == 0 {
                    //create header
                    //payload length
                    self.m_header[0] = (self.m_payload_len >> 4) as u8;
                    self.m_header[1] = (self.m_payload_len & 0x0F) as u8;
                    //coding rate and has_crc
                    self.m_header[2] = (self.m_cr << 1) | (self.m_has_crc as u8);
                    //header checksum
                    let c4 = (self.m_header[0] & 0b1000) >> 3
                        ^ (self.m_header[0] & 0b0100) >> 2
                        ^ (self.m_header[0] & 0b0010) >> 1
                        ^ (self.m_header[0] & 0b0001);
                    let c3 = (self.m_header[0] & 0b1000) >> 3
                        ^ (self.m_header[1] & 0b1000) >> 3
                        ^ (self.m_header[1] & 0b0100) >> 2
                        ^ (self.m_header[1] & 0b0010) >> 1
                        ^ (self.m_header[2] & 0b0001);
                    let c2 = (self.m_header[0] & 0b0100) >> 2
                        ^ (self.m_header[1] & 0b1000) >> 3
                        ^ (self.m_header[1] & 0b0001)
                        ^ (self.m_header[2] & 0b1000) >> 3
                        ^ (self.m_header[2] & 0b0010) >> 1;
                    let c1 = (self.m_header[0] & 0b0010) >> 1
                        ^ (self.m_header[1] & 0b0100) >> 2
                        ^ (self.m_header[1] & 0b0001)
                        ^ (self.m_header[2] & 0b0100) >> 2
                        ^ (self.m_header[2] & 0b0010) >> 1
                        ^ (self.m_header[2] & 0b0001);
                    let c0 = (self.m_header[0] & 0b0001)
                        ^ (self.m_header[1] & 0b0010) >> 1
                        ^ (self.m_header[2] & 0b1000) >> 3
                        ^ (self.m_header[2] & 0b0100) >> 2
                        ^ (self.m_header[2] & 0b0010) >> 1
                        ^ (self.m_header[2] & 0b0001);
                    self.m_header[3] = c4;
                    self.m_header[4] = c3 << 3 | c2 << 2 | c1 << 1 | c0;
                    //add tag
                    sio.output(0).add_tag(
                        0,
                        Tag::NamedAny(
                            "frame_len".to_string(),
                            Box::new(Pmt::Usize(self.m_tag_payload_len)),
                        ),
                    );
                    sio.output(0).add_tag(
                        0,
                        Tag::NamedAny(
                            "payload_str".to_string(),
                            Box::new(Pmt::Blob(self.m_tag_payload_str.clone())),
                        ),
                    );
                }
                for out_slot in out.iter_mut().take(nitems_to_process) {
                    if self.m_cnt_header_nibbles < 5 {
                        *out_slot = self.m_header[self.m_cnt_header_nibbles];
                        self.m_cnt_header_nibbles += 1;
                        out_offset += 1;
                    } else {
                        break;
                    }
                }
            }
            if self.m_impl_head && self.m_cnt_nibbles == 0 {
                //add tag
                sio.output(0).add_tag(
                    0,
                    Tag::NamedAny(
                        "frame_len".to_string(),
                        Box::new(Pmt::Usize(self.m_tag_payload_len)),
                    ),
                );
                sio.output(0).add_tag(
                    0,
                    Tag::NamedAny(
                        "payload_str".to_string(),
                        Box::new(Pmt::Blob(self.m_tag_payload_str.clone())),
                    ),
                );
            }
            for i in out_offset..nitems_to_process {
                out[i] = input[i - out_offset];
                self.m_cnt_nibbles += 1;
                self.m_cnt_header_nibbles = 0;
            }
            // info! {"Header: producing {} samples.", nitems_to_process};
            sio.input(0).consume(nitems_to_process - out_offset);
            sio.output(0).produce(nitems_to_process);
        }
        Ok(())
    }
}

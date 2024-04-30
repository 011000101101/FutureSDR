use futuresdr::anyhow::Result;
use futuresdr::macros::{async_trait, message_handler};
use std::cmp::min;

// use futuresdr::futures::FutureExt;

use futuresdr::log::warn;

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
use futuresdr::runtime::{Block, ItemTag};

pub struct AddCrc {
    m_has_crc: bool,      //indicate the presence of a payload CRC
    m_payload: Vec<u8>,   // payload data
    m_payload_len: usize, // length of the payload in Bytes
    m_frame_len: usize,   // length of the frame in number of gnuradio items
    m_cnt: usize,         // counter of the number of symbol in frame
    flush: bool,
}

impl AddCrc {
    pub fn new(has_crc: bool) -> Block {
        Block::new(
            BlockMetaBuilder::new("AddCrc").build(),
            StreamIoBuilder::new()
                .add_input::<u8>("in")
                .add_output::<u8>("out")
                .build(),
            MessageIoBuilder::new()
                .add_input("flush_queue", Self::flush_queue)
                .add_output("flush_propagate")
                .build(),
            AddCrc {
                m_has_crc: has_crc,
                m_payload: vec![],
                m_payload_len: 0, // implicit
                m_frame_len: 0,
                m_cnt: 0,
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

    fn crc16(crc_value_in: u16, new_byte_tmp: u8) -> u16 {
        let mut crc_value = crc_value_in;
        let mut new_byte = new_byte_tmp as u16;
        for _i in 0..8 {
            if ((crc_value & 0x8000) >> 8) ^ (new_byte & 0x80) != 0 {
                crc_value = (crc_value << 1) ^ 0x1021;
            } else {
                crc_value <<= 1;
            }
            new_byte <<= 1;
        }
        crc_value
    }
}

#[async_trait]
impl Kernel for AddCrc {
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
            self.m_payload = vec![];
            self.m_payload_len = 0;
            self.m_frame_len = 0;
            self.m_cnt = 0;
            self.flush = false;
            io.call_again = true;
            return Ok(());
        }

        let out = sio.output(0).slice::<u8>();
        let noutput_items = out.len().saturating_sub(4);
        let mut nitems_to_process = min(input.len(), noutput_items);
        // info! {"AddCrc: Flag 1 - nitems_to_process: {}", nitems_to_process};
        // info! {"AddCrc: Flag 1 - noutput_items: {}", noutput_items};
        let tags: Vec<(usize, Vec<u8>)> = sio
            .input(0)
            .tags()
            .iter()
            .filter_map(|x| match x {
                ItemTag {
                    index,
                    tag: Tag::NamedAny(n, val),
                } => {
                    if n == "payload_str" {
                        match (**val).downcast_ref().unwrap() {
                            Pmt::Blob(payload) => Some((*index, payload.clone())),
                            _ => None,
                        }
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .collect();
        // info! {"AddCrc: {:?}", tags};
        if !tags.is_empty() {
            if tags[0].0 != 0 {
                nitems_to_process = min(tags[0].0, noutput_items);
                // info! {"AddCrc: Flag 2 - nitems_to_process: {}", nitems_to_process};
            } else {
                if tags.len() >= 2 {
                    nitems_to_process = min(tags[1].0, noutput_items);
                    // info! {"AddCrc: Flag 3 - nitems_to_process: {}", nitems_to_process};
                }
                self.m_payload = tags[0].1.clone();
                //pass tags downstream
                if nitems_to_process > 0 {
                    let tags_tmp: Vec<(usize, usize)> = sio
                        .input(0)
                        .tags()
                        .iter()
                        .filter_map(|x| match x {
                            ItemTag {
                                index,
                                tag: Tag::NamedAny(n, val),
                            } => {
                                if n == "frame_len" {
                                    match (**val).downcast_ref().unwrap() {
                                        Pmt::Usize(frame_len) => Some((*index, *frame_len)),
                                        _ => None,
                                    }
                                } else {
                                    None
                                }
                            }
                            _ => None,
                        })
                        .collect();
                    if !tags_tmp.is_empty() {
                        self.m_frame_len = tags_tmp[0].1;
                        sio.output(0).add_tag(
                            0,
                            Tag::NamedAny(
                                "frame_len".to_string(),
                                Box::new(Pmt::Usize(
                                    self.m_frame_len + if self.m_has_crc { 4 } else { 0 },
                                )),
                            ),
                        );
                    }
                }
                self.m_cnt = 0;
            }
        }
        if nitems_to_process == 0 {
            if out.is_empty() {
                warn!("AddCrc: no space in output buffer, waiting for more.");
            }
            // else {
            //     warn!("AddCrc: no samples in input buffer, waiting for more.");
            // }
            return Ok(());
        }
        self.m_cnt += nitems_to_process;
        let nitems_to_produce = if self.m_has_crc && self.m_cnt == self.m_frame_len {
            //append the CRC to the payload
            let mut crc: u16 = 0x0000;
            self.m_payload_len = self.m_payload.len();
            //calculate CRC on the N-2 firsts data bytes using Poly=1021 Init=0000
            for i in 0..(self.m_payload_len - 2) {
                crc = Self::crc16(crc, self.m_payload[i]);
            }
            //XOR the obtained CRC with the last 2 data bytes
            crc = crc
                ^ (self.m_payload[self.m_payload_len - 1] as u16)
                ^ ((self.m_payload[self.m_payload_len - 2] as u16) << 8);
            //Place the CRC in the correct output nibble
            out[nitems_to_process] = (crc & 0x000F) as u8;
            out[nitems_to_process + 1] = ((crc & 0x00F0) >> 4) as u8;
            out[nitems_to_process + 2] = ((crc & 0x0F00) >> 8) as u8;
            out[nitems_to_process + 3] = ((crc & 0xF000) >> 12) as u8;
            self.m_payload = vec![];
            nitems_to_process + 4
        } else {
            nitems_to_process
        };
        // if nitems_to_produce > 0 {
        //     info! {"AddCrc: producing {} samples.", nitems_to_produce};
        // }
        out[0..nitems_to_process].copy_from_slice(&input[0..nitems_to_process]);
        sio.input(0).consume(nitems_to_process);
        sio.output(0).produce(nitems_to_produce);
        Ok(())
    }
}

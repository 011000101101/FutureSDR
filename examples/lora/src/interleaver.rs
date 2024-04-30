use futuresdr::anyhow::Result;
use futuresdr::log::warn;
use futuresdr::macros::{async_trait, message_handler};
use std::cmp::{max, min};

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

use crate::utilities::*;

pub struct Interleaver {
    m_cr: usize,        // Transmission coding rate
    m_sf: usize,        // Transmission spreading factor
    cw_cnt: usize,      // count the number of codewords
    m_frame_len: usize, //length of the frame in number of items
    m_ldro: bool,       // use the low datarate optimisation mode
                        // m_bw: usize,
    flush: bool,
}

impl Interleaver {
    pub fn new(cr: usize, sf: usize, ldro: usize, bw: usize) -> Block {
        Block::new(
            BlockMetaBuilder::new("Interleaver").build(),
            StreamIoBuilder::new()
                .add_input::<u8>("in")
                .add_output::<u16>("out")
                .build(),
            MessageIoBuilder::new()
                .add_input("flush_queue", Self::flush_queue)
                .add_output("flush_propagate")
                .build(),
            Interleaver {
                m_sf: sf,
                m_cr: cr,
                // m_bw: bw,
                m_ldro: if LdroMode::from(ldro) == LdroMode::AUTO {
                    ((1 << sf) as f32) * 1.0e3 / (bw as f32) > LDRO_MAX_DURATION_MS
                } else {
                    ldro != 0
                },
                cw_cnt: 0,
                m_frame_len: 0, // implicit
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

    // fn set_cr(&mut self, cr: usize) {
    //     self.m_cr = cr;
    // }
    //
    // fn set_sf(&mut self, sf: usize) {
    //     self.m_sf = sf;
    // }
    //
    // fn get_cr(&self) -> usize {
    //     self.m_cr
    // }
}

#[async_trait]
impl Kernel for Interleaver {
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
            self.cw_cnt = 0;
            self.m_frame_len = 0;
            self.flush = false;
            io.call_again = true;
            return Ok(());
        }

        let out = sio.output(0).slice::<u16>();
        let mut nitems_to_process = input.len();
        // read tags
        let tags: Vec<(usize, usize)> = sio
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
        if !tags.is_empty() {
            if tags[0].0 != 0 {
                nitems_to_process = tags[0].0;
            } else {
                if tags.len() >= 2 {
                    nitems_to_process = tags[1].0;
                }
                self.cw_cnt = 0;
                self.m_frame_len = tags[0].1;
            }
        }
        // handle the first interleaved block special case
        let cw_len: usize = 4 + if self.cw_cnt < self.m_sf - 2 {
            4
        } else {
            self.m_cr
        };
        let sf_app: usize = if (self.cw_cnt < self.m_sf - 2) || self.m_ldro {
            self.m_sf - 2
        } else {
            self.m_sf
        };
        nitems_to_process = min(nitems_to_process, sf_app);
        // info!("sf_app: {}", sf_app);
        // info!("nitems_to_process: {}", nitems_to_process);
        if nitems_to_process > 0 {
            if out.len() < cw_len {
                warn!("Interleaver: not enough space in output buffer for one interleaved block, waiting for more.");
                return Ok(());
            }
            let nitems_to_consume = min(sf_app, nitems_to_process);
            if self.m_frame_len != 0
                && (nitems_to_process >= sf_app
                    || self.cw_cnt + nitems_to_process == self.m_frame_len)
            {
                //propagate tag
                if self.cw_cnt == 0 {
                    // info!("self.m_frame_len: {}", self.m_frame_len);
                    // info!("self.m_sf: {}", self.m_sf);
                    sio.output(0).add_tag(
                        0,
                        Tag::NamedAny(
                            "frame_len".to_string(),
                            Box::new(Pmt::Usize(
                                8 + max(
                                    ((self.m_frame_len - self.m_sf + 2) as f64
                                        / (self.m_sf - if self.m_ldro { 1 } else { 0 }) as f64)
                                        .ceil() as usize
                                        * (self.m_cr + 4),
                                    0,
                                ), //get number of items in frame
                            )),
                        ),
                    );
                }
                //         //Create the empty matrices
                //         std::vector<std::vector<bool>> cw_bin(sf_app);
                let init_bit: Vec<bool> = vec![false; self.m_sf];
                let mut inter_bin: Vec<Vec<bool>> = vec![init_bit; cw_len];

                //convert to input codewords to binary vector of vector
                let cw_bin: Vec<Vec<bool>> = if nitems_to_consume < sf_app {
                    input[0..nitems_to_consume]
                        .iter()
                        .chain(vec![0_u8; sf_app - nitems_to_consume].iter())
                        .enumerate()
                        .map(|(i, x)| {
                            if i >= nitems_to_consume {
                                int2bool(0, cw_len)
                            } else {
                                int2bool(*x as u16, cw_len)
                            }
                        })
                        .collect()
                } else {
                    input[0..sf_app]
                        .iter()
                        .enumerate()
                        .map(|(i, x)| {
                            if i >= nitems_to_consume {
                                int2bool(0, cw_len)
                            } else {
                                int2bool(*x as u16, cw_len)
                            }
                        })
                        .collect()
                };

                self.cw_cnt += sf_app;
                // #ifdef GRLORA_DEBUG
                //         std::cout << "codewords---- " << std::endl;
                //         for (uint32_t i = 0u; i < sf_app; i++)
                //         {
                //           for (int j = 0; j < int(cw_len); j++)
                //           {
                //             std::cout << cw_bin[i][j];
                //           }
                //           std::cout << " 0x" << std::hex << (int)in[i] << std::dec << std::endl;
                //         }
                //         std::cout << std::endl;
                // #endif

                //Do the actual interleaving
                for i in 0..cw_len {
                    for j in 0..sf_app {
                        inter_bin[i][j] = cw_bin[my_modulo(i as isize - j as isize - 1, sf_app)][i];
                    }
                    //For the first bloc we add a parity bit and a zero in the end of the lora symbol(reduced rate)
                    if (self.cw_cnt == self.m_sf - 2) || self.m_ldro {
                        inter_bin[i][sf_app] = inter_bin[i]
                            .iter()
                            .fold(0, |acc, e| acc + if *e { 1 } else { 0 })
                            % 2
                            != 0;
                    }
                    out[i] = bool2int(&inter_bin[i]);
                }
                // #ifdef GRLORA_DEBUG
                //         std::cout << "interleaved------" << std::endl;
                //         for (uint32_t i = 0u; i < cw_len; i++)
                //         {
                //           for (int j = 0; j < int(m_sf); j++)
                //           {
                //             std::cout << inter_bin[i][j];
                //           }
                //           std::cout << " " << out[i] << std::endl;
                //         }
                //         std::cout << std::endl;
                // #endif
                // info! {"Interleaver: producing {} samples.", cw_len};
                sio.input(0).consume(nitems_to_consume);
                sio.output(0).produce(cw_len);
            } else {
                warn!("Interleaver: not enough samples in input buffer, waiting for more.");
                return Ok(());
            }
        }
        Ok(())
    }
}

use std::cmp;
use std::fmt;
use std::ptr;
use std::str::FromStr;

use crate::anyhow::Result;
use crate::runtime::Block;
use crate::runtime::BlockMeta;
use crate::runtime::BlockMetaBuilder;
use crate::runtime::Kernel;
use crate::runtime::MessageIo;
use crate::runtime::MessageIoBuilder;
use crate::runtime::Pmt;
use crate::runtime::StreamIo;
use crate::runtime::StreamIoBuilder;
use crate::runtime::WorkIo;
use crate::runtime::ItemTag;
use crate::runtime::Tag;

/// Drop Policy for [`Selector`] block
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropPolicy {
    /// Drop all unselected inputs
    /// Warning: probably your flowgraph at inputs should be rate-limited somehow.
    DropAll,

    /// Drop unselected inputs at the same rate as the selected one.
    /// Warning: probably you will use more CPU than needed,
    /// but get a consistent CPU usage whatever the select
    SameRate,

    /// Do not drop inputs that are unselected.
    NoDrop,
}

impl FromStr for DropPolicy {
    type Err = String;

    fn from_str(s: &str) -> Result<DropPolicy, Self::Err> {
        match s {
            "same" => Ok(DropPolicy::SameRate),
            "same-rate" => Ok(DropPolicy::SameRate),
            "SAME" => Ok(DropPolicy::SameRate),
            "SAME_RATE" => Ok(DropPolicy::SameRate),
            "sameRate" => Ok(DropPolicy::SameRate),

            "none" => Ok(DropPolicy::NoDrop),
            "NoDrop" => Ok(DropPolicy::NoDrop),
            "NO_DROP" => Ok(DropPolicy::NoDrop),
            "no-drop" => Ok(DropPolicy::NoDrop),

            "all" => Ok(DropPolicy::DropAll),
            "DropAll" => Ok(DropPolicy::DropAll),
            "drop-all" => Ok(DropPolicy::DropAll),
            "DROP_ALL" => Ok(DropPolicy::DropAll),

            _ => Err("String didn't match value".to_string()),
        }
    }
}

impl fmt::Display for DropPolicy {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            DropPolicy::NoDrop => write!(f, "NoDrop"),
            DropPolicy::DropAll => write!(f, "DropAll"),
            DropPolicy::SameRate => write!(f, "SameRate"),
        }
    }
}

#[derive(PartialEq, Eq)]
enum State {
    FrameStart,
    Copy(usize),
    Flush
}

/// Forward the input stream with a given index to the output stream with a
/// given index.
pub struct Selector<A, const N: usize, const M: usize>
where
    A: Send + 'static + Copy,
{
    input_index: usize,
    output_index: usize,
    drop_policy: DropPolicy,
    _p1: std::marker::PhantomData<A>,
    state: State,
}

impl<A, const N: usize, const M: usize> Selector<A, N, M>
where
    A: Send + 'static + Copy,
{
    /// Create Selector block
    pub fn new(drop_policy: DropPolicy) -> Block {
        let mut stream_builder = StreamIoBuilder::new();
        for i in 0..N {
            stream_builder = stream_builder.add_input::<A>(format!("in{i}").as_str());
        }
        for i in 0..M {
            stream_builder = stream_builder.add_output::<A>(format!("out{i}").as_str());
        }
        Block::new(
            BlockMetaBuilder::new(format!("Selector<{N}, {M}>")).build(),
            stream_builder.build(),
            MessageIoBuilder::<Self>::new()
                .add_input("input_index", Self::input_index)
                .add_input("output_index", Self::output_index)
                .add_input("flush", Self::flush_handler)
                .build(),
            Selector {
                input_index: 0,
                output_index: 0,
                drop_policy,
                _p1: std::marker::PhantomData,
                state: State::FrameStart,
            },
        )
    }

    #[message_handler]
    async fn flush_handler(
        &mut self,
        io: &mut WorkIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
        _p: Pmt,
    ) -> Result<Pmt> {
        self.state = State::Flush;
        io.call_again = true;
        Ok(Pmt::Null)
    }

    #[message_handler]
    async fn input_index(
        &mut self,
        _io: &mut WorkIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
        p: Pmt,
    ) -> Result<Pmt> {
        match p {
            Pmt::U32(v) => self.input_index = (v as usize) % N,
            Pmt::U64(v) => self.input_index = (v as usize) % N,
            _ => todo!(),
        }
        Ok(Pmt::U32(self.input_index as u32))
    }

    #[message_handler]
    async fn output_index(
        &mut self,
        _io: &mut WorkIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
        p: Pmt,
    ) -> Result<Pmt> {
        match p {
            Pmt::U32(v) => self.output_index = (v as usize) % M,
            Pmt::U64(v) => self.output_index = (v as usize) % M,
            _ => todo!(),
        }
        Ok(Pmt::U32(self.output_index as u32))
    }
}

#[doc(hidden)]
#[async_trait]
impl<A, const N: usize, const M: usize> Kernel for Selector<A, N, M>
where
    A: Send + 'static + Copy,
{
    async fn work(
        &mut self,
        io: &mut WorkIo,
        sio: &mut StreamIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
    ) -> Result<()> {
        let item_size = std::mem::size_of::<A>();

        // let m = cmp::min(i.len(), o.len());
        // if m > 0 {
        //     unsafe {
        //         ptr::copy_nonoverlapping(i.as_ptr(), o.as_mut_ptr(), m);
        //     }
        //     //     for (v, r) in i.iter().zip(o.iter_mut()) {
        //     //         *r = *v;
        //     //     }
        //
        // }



        // TODO handle item_size correctly
        let i = sio.input(self.input_index).slice_unchecked::<A>();
        let o = sio.output(self.output_index).slice_unchecked::<A>();

        let mut consumed = 0;
        let mut produced = 0;

        while produced < o.len() {
            match self.state {
                State::FrameStart => {
                    // println!("www i.len() {}", i.len());
                    // println!("www o.len() {}", o.len());
                    // println!("www produced {}", produced);
                    // println!("www consumed {}", consumed);
                    // println!("www {:?}", sio
                    //     .input(self.input_index)
                    //     .tags().clone());
                    if consumed == i.len() || produced == o.len() {
                        break;
                    } else if let Some(ItemTag {
                                    tag: Tag::NamedUsize(name, burst_size), index: burst_start_index
                                }) = sio
                        .input(self.input_index)
                        .tags()
                        .iter()
                        .next()
                        .cloned()
                    {
                        assert_eq!(name, "burst_start");
                        if burst_start_index > consumed {
                            // forward all untagged samples in non-burst mode up to the beginning of the found burst tag.
                            let n_to_produce = cmp::min(burst_start_index - consumed, cmp::min(i.len() - consumed, o.len() - produced));
                            assert!(n_to_produce != 0);
                            unsafe {
                                ptr::copy_nonoverlapping(i.as_ptr().add(consumed), o.as_mut_ptr().add(produced), n_to_produce);
                            }
                            produced += n_to_produce;
                            consumed += n_to_produce;
                            if sio.inputs().len() > 1 {
                                warn!("sending {} untagged samples preceding tagged burst, but expected only tagged burst-mode data on outgoing flowgraph..", burst_start_index - consumed);
                                // panic!();
                            }
                        }
                        let burst_size = if burst_start_index < consumed {
                            warn!("burst start tag points to already consumed sample: {} ({} consumed)", burst_start_index, consumed);
                            burst_size - (consumed - burst_start_index)
                        } else {burst_size};
                        self.state = State::Copy(burst_size);
                        sio.output(0).add_tag(
                            produced,
                            Tag::NamedUsize(
                                "burst_start".to_string(),
                                burst_size,
                            ),
                        );
                    } else {
                        // no tag in input stream: send all samples in non-burst mdoe
                        if sio.inputs().len() > 1 {
                            warn!("forwarding {} samples in non-burst mode, but expected only tagged burst-mode data on outgoing flowgraph.", i.len() - consumed);
                            // panic!();
                        }
                        // panic!("no frame start tag");
                        let n_to_produce = cmp::min(i.len() - consumed, o.len() - produced);
                        assert!(n_to_produce != 0);
                        unsafe {
                            ptr::copy_nonoverlapping(i.as_ptr().add(consumed), o.as_mut_ptr().add(produced), n_to_produce);
                        }
                        produced += n_to_produce;
                        consumed += n_to_produce;
                        break;  // depleted available input or output buffer, wait for new data
                    }
                }
                State::Copy(left) => {
                    // copy burst of known size
                    if left == 0 {
                        self.state = State::FrameStart;
                        break;
                    } else if consumed == i.len() || produced == o.len() {
                        break;
                    } else {
                        let n_to_produce = cmp::min(cmp::min(i.len() - consumed, o.len() - produced), left);
                        assert!(n_to_produce != 0);
                        unsafe {
                            ptr::copy_nonoverlapping(i.as_ptr().add(consumed), o.as_mut_ptr().add(produced), n_to_produce);
                        }
                        produced += n_to_produce;
                        consumed += n_to_produce;
                        self.state = State::Copy(left - n_to_produce);
                    }
                }
                State::Flush => {
                    sio.input(self.input_index).consume(i.len());
                    self.state = State::FrameStart;
                    io.call_again = true;
                    return Ok(());
                }
            }
        }

        // println!("ddd produced {}", produced);
        // println!("ddd consumed {}", consumed);
        sio.input(self.input_index).consume(consumed);
        sio.output(self.output_index).produce(produced);


        if self.drop_policy != DropPolicy::NoDrop {
            let nb_drop = if self.drop_policy == DropPolicy::SameRate {
                consumed / item_size // Drop at the same rate as the selected one
            } else {
                std::usize::MAX // Drops all other inputs
            };
            for i in 0..N {
                if i != self.input_index {
                    let input = sio.input(i).slice::<A>();
                    sio.input(i).consume(input.len().min(nb_drop));
                }
            }
        }

        // Maybe this should be configurable behaviour? finish on current finish? when all input have finished?
        if sio.input(self.input_index).finished() && consumed == i.len() {
            io.finished = true;
        }

        Ok(())
    }
}

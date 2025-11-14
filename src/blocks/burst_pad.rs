use crate::runtime::BlockMeta;
use crate::runtime::BlockMetaBuilder;
use crate::runtime::ItemTag;
use crate::runtime::Kernel;
use crate::runtime::MessageIo;
use crate::runtime::MessageIoBuilder;
use crate::runtime::StreamIo;
use crate::runtime::StreamIoBuilder;
use crate::runtime::Tag;
use crate::runtime::TypedBlock;
use crate::runtime::WorkIo;

enum BurstPadState {
    Copy(usize, bool),
    PadHead(usize, usize, bool),
    PadTail(usize),
    HaveUntaggedSamples(usize),
    ClaimPending,
}

/// Pad head and/or tail of bursts by a fixed amount of samples, extending the burst tag.
pub struct BurstPad<T: core::marker::Copy + Send + 'static> {
    state: BurstPadState,
    num_samples_pad_head: usize,
    num_samples_pad_tail: usize,
    pad_value: T,
}

impl<T: core::marker::Copy + Send + 'static> BurstPad<T> {
    /// Create [`struct@futuresdr::blocks::CopyAndTag`] block
    pub fn new(num_samples_head: usize, num_samples_tail: usize, value: T) -> TypedBlock<Self> {
        TypedBlock::new(
            BlockMetaBuilder::new("BurstPad").build(),
            StreamIoBuilder::new()
                .add_input::<T>("in")
                .add_output_with_size::<T>("out", 131072)
                .build(),
            MessageIoBuilder::<Self>::new().build(),
            BurstPad::<T> {
                state: BurstPadState::ClaimPending,
                num_samples_pad_head: num_samples_head,
                num_samples_pad_tail: num_samples_tail,
                pad_value: value,
            },
        )
    }
}

#[doc(hidden)]
#[async_trait]
impl<T: core::marker::Copy + Send + 'static> Kernel for BurstPad<T> {
    async fn work(
        &mut self,
        io: &mut WorkIo,
        sio: &mut StreamIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
    ) -> crate::runtime::Result<()> {
        let mut consumed = 0;
        let mut produced = 0;

        let num_samples_in_input = sio.input(0).slice::<T>().len();
        let num_slots_in_output = sio.output(0).slice::<T>().len();

        'outer: loop {
            match self.state {
                BurstPadState::Copy(num_samples_left, tag_pending) => {
                    let n_consume = (num_samples_in_input - consumed).min(num_samples_left);
                    let n_produce = (num_slots_in_output - produced).min(num_samples_left);
                    let n_copy = n_consume.min(n_produce);
                    if n_copy > 0 {
                        if tag_pending {
                            sio.output(0).add_tag(
                                produced,
                                Tag::NamedUsize(
                                    "burst_start".to_string(),
                                    num_samples_left + self.num_samples_pad_tail,
                                ),
                            );
                        }
                        let i = sio.input(0).slice::<T>();
                        let o = sio.output(0).slice::<T>();
                        o[produced..produced + n_copy]
                            .copy_from_slice(&i[consumed..consumed + n_copy]);
                        consumed += n_copy;
                        produced += n_copy;
                        if n_copy == num_samples_left {
                            if self.num_samples_pad_tail > 0 {
                                self.state = BurstPadState::PadTail(self.num_samples_pad_tail);
                            } else {
                                self.state = BurstPadState::ClaimPending;
                            }
                        } else {
                            self.state = BurstPadState::Copy(num_samples_left - n_copy, false);
                        }
                    } else {
                        break 'outer;
                    }
                }
                BurstPadState::PadHead(num_samples_left, original_burst_len, tag_pending) => {
                    let n_produce = (num_slots_in_output - produced).min(num_samples_left);
                    if n_produce > 0 {
                        if tag_pending {
                            sio.output(0).add_tag(
                                produced,
                                Tag::NamedUsize(
                                    "burst_start".to_string(),
                                    num_samples_left
                                        + original_burst_len
                                        + self.num_samples_pad_tail,
                                ),
                            );
                        }
                        let o = sio.output(0).slice::<T>();
                        for slot in o.iter_mut().skip(produced).take(n_produce) {
                            *slot = self.pad_value;
                        }
                        produced += n_produce;
                        if n_produce == num_samples_left {
                            self.state = BurstPadState::Copy(original_burst_len, false);
                        } else {
                            self.state = BurstPadState::PadHead(
                                num_samples_left - n_produce,
                                original_burst_len,
                                false,
                            );
                        }
                    } else {
                        break 'outer;
                    }
                }
                BurstPadState::PadTail(num_samples_left) => {
                    let n_produce = (num_slots_in_output - produced).min(num_samples_left);
                    if n_produce > 0 {
                        let o = sio.output(0).slice::<T>();
                        for slot in o.iter_mut().skip(produced).take(n_produce) {
                            *slot = self.pad_value;
                        }
                        produced += n_produce;
                        if n_produce == num_samples_left {
                            self.state = BurstPadState::ClaimPending;
                        } else {
                            self.state = BurstPadState::PadTail(num_samples_left - n_produce);
                        }
                    } else {
                        break 'outer;
                    }
                }
                BurstPadState::HaveUntaggedSamples(num_samples_left) => {
                    self.state = BurstPadState::Copy(num_samples_left, false);
                    // TODO this just copies
                }
                BurstPadState::ClaimPending => {
                    // get new state depending on available input
                    if num_samples_in_input - consumed == 0 {
                        break 'outer;
                    } else {
                        self.state = sio
                            .input(0)
                            .tags()
                            .iter()
                            .find_map(|x| match x {
                                ItemTag {
                                    index,
                                    tag: Tag::NamedUsize(n, len),
                                } => {
                                    if n == "burst_start" {
                                        if *index == 0 {
                                            if self.num_samples_pad_head > 0 {
                                                Some(BurstPadState::PadHead(
                                                    self.num_samples_pad_head,
                                                    *len,
                                                    true,
                                                ))
                                            } else {
                                                Some(BurstPadState::Copy(*len, true))
                                            }
                                        } else {
                                            Some(BurstPadState::HaveUntaggedSamples(*index))
                                        }
                                    } else {
                                        None
                                    }
                                }
                                _ => None,
                            })
                            .unwrap_or(BurstPadState::HaveUntaggedSamples(
                                num_samples_in_input - consumed,
                            ));
                    }
                }
            }
        }

        if consumed > 0 {
            // debug!("consumed {consumed} samples");
            sio.input(0).consume(consumed);
        }
        if produced > 0 {
            // debug!("produced {produced} samples");
            sio.output(0).produce(produced);
        }

        if sio.input(0).finished() && consumed == num_samples_in_input {
            io.finished = true;
        };

        Ok(())
    }
}

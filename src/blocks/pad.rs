use crate::runtime::BlockMeta;
use crate::runtime::BlockMetaBuilder;
use crate::runtime::Error;
use crate::runtime::ItemTag;
use crate::runtime::Kernel;
use crate::runtime::MessageIo;
use crate::runtime::MessageIoBuilder;
use crate::runtime::Result;
use crate::runtime::StreamIo;
use crate::runtime::StreamIoBuilder;
use crate::runtime::Tag;
use crate::runtime::TypedBlock;
use crate::runtime::WorkIo;
use num_complex::Complex32;
use wgpu::naga::proc::ConstantEvaluatorError::NotImplemented;

/// Pad sample bursts to the specified size (e.g. to fill minimum transmit buffer size for device sink)
/// Optionally copy or discard untagged samples
///
/// Treats every tagged burst as an independent unit and pads it either to a minimum size of N (if the burst contains fewer samples) or to a multiple of N
///
/// # Stream Inputs
///
/// `in`: Input, expected to contain "burst_start" named_usize tags
///
/// # Stream Outputs
///
/// `out`: Output, potentially padded copy the input
///
/// # Usage
/// TODO

enum InputStateTracker {
    HaveBurst(usize, usize, Option<Tag>), // burst can be transmitted in chunks as long as there is enough space in out buffer, will start to transmit as many buffers as the output can fit
    // propagating the burst_start tag ensures the following blocks handle the samples as a unit, and the output buffer size requirement ensures the flowgraph does not stall on large bursts
    HaveUntaggedSamples(usize),
    InputEmpty,
    ClaimPending,
}

trait SampleBatchState {
    fn samples_remaining(&self) -> usize;
    fn transmit(&mut self, num_samples: usize);
}

struct BurstState {
    len: usize,
    already_transmitted: usize,
}

impl SampleBatchState for InputStateTracker {
    fn samples_remaining(&self) -> usize {
        match self {
            Self::HaveBurst(len, already_tranmitted, _) => len - already_tranmitted,
            Self::HaveUntaggedSamples(num_samples_remaining) => *num_samples_remaining,
            Self::InputEmpty | Self::ClaimPending => 0,
        }
    }

    fn transmit(&mut self, num_samples: usize) -> Result<()> {
        match self {
            Self::HaveBurst(len, already_tranmitted, _) => *already_tranmitted += num_samples,
            Self::HaveUntaggedSamples(num_samples_remaining) => {
                *num_samples_remaining -= num_samples
            }
            Self::InputEmpty | Self::ClaimPending => {
                Err(Error::RuntimeError("Invalid State.".to_string()))?
            }
        }
        Ok(())
    }
}

struct UntaggedSamplesState {
    num_samples: usize,
}

impl SampleBatchState for UntaggedSamplesState {
    fn samples_remaining(&self) -> usize {
        self.num_samples
    }

    fn transmit(&mut self, num_samples: usize) {
        self.num_samples -= num_samples;
    }
}

enum PadderUntaggedSamplesPolicy {
    /// drop all untagged samples
    Drop,
    /// copy all available samples and afterwards pad the last block. Essentially treats the exact amount of untagged samples available at the input as a single tagged burst.
    CopyAndPad,
    /// simply copy available samples in blocks until a block can no longer be filled completely, then wait for more samples. May fragment untagged frames, if there is space in the output buffer and the Device underflows.
    CopyAndHold,
}

pub struct ChunkAndPad<T: core::marker::Copy + Send + Default + 'static> {
    block_size: usize,
    untagged_samples_policy: PadderUntaggedSamplesPolicy,
    state: InputStateTracker,
    _type: std::marker::PhantomData<T>,
}

impl<T: core::marker::Copy + Send + Default + 'static> ChunkAndPad<T> {
    /// Create [`struct@Copy`] block
    pub fn new(block_size: usize, drop_untagged: bool) -> TypedBlock<Self> {
        TypedBlock::new(
            BlockMetaBuilder::new("ChunkAndPad").build(),
            StreamIoBuilder::new()
                .add_input_with_size::<T>("in", block_size)
                .add_output_with_relative_size::<T>(
                    "out",
                    &(|input_sizes: &[usize]| {
                        (*input_sizes.iter().max().unwrap_or(&block_size) as f64
                            / block_size as f64)
                            .ceil() as usize
                            * block_size
                    }),
                )
                .build(),
            MessageIoBuilder::<Self>::new().build(),
            ChunkAndPad::<T> {
                block_size,
                drop_untagged,
                current_burst_len: None,
                burst_partially_transmitted: false,
                counter: 0,
                _type: std::marker::PhantomData,
            },
        )
    }
}

#[doc(hidden)]
#[async_trait]
impl<T: core::marker::Copy + Send + Default + 'static> Kernel for ChunkAndPad<T> {
    async fn work(
        &mut self,
        io: &mut WorkIo,
        sio: &mut StreamIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
    ) -> Result<()> {
        let mut consumed = 0;
        let mut produced = 0;

        let num_samples_in_input = sio.input(0).slice::<T>().len();
        let num_slots_in_output = sio.output(0).slice::<T>().len();

        // if num_samples_in_input == 0 || num_samples_in_output == 0 {
        //     if sio.input(0).finished() {
        //         io.finished = true;
        //     }
        //     return Ok(());
        // }

        'outer: while num_slots_in_output - produced >= self.block_size {
            let mut num_samples_in_current_block: usize = 0;
            let end_of_current_block = produced + self.block_size;
            while num_samples_in_current_block < self.block_size {
                let num_samples_in_input_remaining = num_samples_in_input - consumed;
                let num_slots_in_block_remaining = self.block_size - num_samples_in_current_block;
                let i = sio.input(0).slice::<T>();
                let o = sio.output(0).slice::<T>();
                match &self.state {
                    InputStateTracker::HaveBurst(len, already_transmitted, original_tag) => {
                        let num_samples_in_batch_remaining = len - already_transmitted;
                        if *already_transmitted > 0 {
                            // burst is ongoing, must not pad in between
                            if num_samples_in_batch_remaining <= num_slots_in_block_remaining
                                && num_samples_in_input_remaining >= num_samples_in_batch_remaining
                            {
                                // can finish current batch
                                let o = sio.output(0).slice::<T>();
                                o[produced..produced + num_samples_in_batch_remaining]
                                    .copy_from_slice(
                                        &i[consumed..consumed + num_samples_in_batch_remaining],
                                    );
                                consumed += num_samples_in_batch_remaining;
                                produced += num_samples_in_batch_remaining;
                                num_samples_in_current_block += num_samples_in_batch_remaining;
                                self.state = InputStateTracker::ClaimPending;
                            } else if num_samples_in_batch_remaining > num_slots_in_block_remaining
                                && num_samples_in_input_remaining >= num_slots_in_block_remaining
                            {
                                // can fill current block with samples from current batch
                                let o = sio.output(0).slice::<T>();
                                o[produced..end_of_current_block]
                                    .copy_from_slice(&i[consumed..end_of_current_block]);
                                consumed += num_slots_in_block_remaining;
                                produced += num_slots_in_block_remaining;
                                num_samples_in_current_block += num_slots_in_block_remaining;
                                self.state = InputStateTracker::HaveBurst(
                                    *len,
                                    already_transmitted + num_slots_in_block_remaining,
                                    original_tag.clone(),
                                );
                            } else {
                                debug_assert!(num_samples_in_current_block == 0);
                                // stall the flowgraph, only produce full blocks
                                break 'outer;
                            }
                        } else {
                            // beginning of new burst, either fill whole block with burst samples or pad the started batch without touching the burst
                        }

                        if num_samples_in_input_remaining < num_slots_in_block_remaining
                            && num_samples_in_input_remaining < num_samples_in_batch_remaining
                        {
                            // fill current block, then exit, keep state (claimed but unused burst) for next iteration
                            debug_assert!(
                                already_transmitted == 0 || num_samples_in_current_block == 0
                            ); // TODO bad
                            let o = sio.output(0).slice::<T>();
                            o[num_samples_in_current_block..self.block_size].fill(T::default());
                            let produced_tmp = self.block_size - num_samples_in_current_block;
                            produced += produced_tmp;
                            num_samples_in_current_block = self.block_size;
                            break 'outer;
                        }
                        if num_samples_in_input_remaining >= num_samples_in_batch_remaining
                            && num_samples_in_batch_remaining <= num_slots_in_block_remaining
                        {
                            // transmit complete remainder of burst
                            o[produced..produced + num_samples_in_batch_remaining].copy_from_slice(
                                &i[consumed..consumed + num_samples_in_batch_remaining],
                            );
                            consumed += num_samples_in_batch_remaining;
                            produced += num_samples_in_batch_remaining;
                            num_samples_in_current_block += num_samples_in_batch_remaining;
                            self.state = InputStateTracker::ClaimPending;
                        } else {
                            // fill remainder of block with samples from this batch, leaving remaining sampleds for next block
                            o[produced..num_slots_in_block_remaining]
                                .copy_from_slice(&i[consumed..num_slots_in_block_remaining]);
                            consumed += num_slots_in_block_remaining;
                            produced += num_slots_in_block_remaining;
                            num_samples_in_current_block += num_slots_in_block_remaining;
                            self.state.transmit(num_slots_in_block_remaining)?;
                        }
                    }
                    InputStateTracker::HaveUntaggedSamples(num_remaining) => {
                        NotImplemented("TODO".to_string());
                    }
                    InputStateTracker::InputEmpty => {
                        // fill current block, then exit
                        let o = sio.output(0).slice::<T>();
                        o[produced..end_of_current_block].fill(T::default());
                        produced = end_of_current_block;
                        num_samples_in_current_block = self.block_size;
                        break 'outer;
                    }
                    InputStateTracker::ClaimPending => {
                        // get new state depending on available input
                        if num_samples_in_input - consumed == 0 {
                            self.state = InputStateTracker::InputEmpty;
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
                                                Some(InputStateTracker::HaveBurst(
                                                    *len,
                                                    0,
                                                    Some(Tag::NamedUsize(n.to_string(), *len)),
                                                ))
                                            } else {
                                                Some(InputStateTracker::HaveUntaggedSamples(*index))
                                            }
                                        } else {
                                            None
                                        }
                                    }
                                    _ => None,
                                })
                                .unwrap_or(InputStateTracker::HaveUntaggedSamples(
                                    num_samples_in_input - consumed,
                                ));
                        }
                    }
                }
            }
        }

        self.update_state(
            &mut state,
            num_samples_remaining_in_input,
            num_slots_remaining_in_output,
            sio,
        );

        match self.current_burst_len {
            Some(remaining_samples) => {
                let tag_to_copy = if !self.burst_partially_transmitted {
                    sio.input(0).tags().iter().find_map(|x| match x {
                        ItemTag {
                            index,
                            tag: Tag::NamedUsize(n, len),
                        } => {
                            if *index == 0 && n == "burst_start" {
                                let new_len = (*len as f64 / self.pad_to as f64).ceil() as usize
                                    * self.pad_to;
                                Some(Tag::NamedUsize("burst_start".to_string(), new_len))
                            } else {
                                None
                            }
                        }
                        _ => None,
                    })
                } else {
                    None
                };
                let i = sio.input(0).slice::<T>();
                let o = sio.output(0).slice::<T>();
                if num_samples_in_output < self.pad_to {
                    // debug!("too few samples: {num_samples_in_output}");
                    return Ok(());
                }
                // from here on we know the output buffer has enough space for a full block of samples
                if remaining_samples >= self.pad_to && num_samples_in_input >= self.pad_to {
                    // copy one block of samples
                    o[..self.pad_to].copy_from_slice(&i[..self.pad_to]);
                    consumed += self.pad_to;
                    produced += self.pad_to;
                    self.current_burst_len = Some(remaining_samples - self.pad_to);
                } else if remaining_samples <= num_samples_in_input {
                    // pad the burst to
                    o[..remaining_samples].copy_from_slice(&i[..remaining_samples]);
                    // unsafe {
                    //     std::ptr::write_bytes(
                    //         o[remaining_samples..].as_mut_ptr(),
                    //         0,
                    //         (self.pad_to - remaining_samples) * std::mem::size_of::<T>(),
                    //     );
                    // }
                    o[remaining_samples..self.pad_to].fill(T::default());
                    consumed += remaining_samples;
                    produced += self.pad_to;
                    self.current_burst_len = None;
                    // self.counter = 1;
                    // io.call_again = true;
                }
                if produced > 0 {
                    sio.output(0)
                        .add_tag(0, Tag::NamedUsize("burst_start".to_string(), self.pad_to));
                    // self.burst_partially_transmitted = true;
                    // if let Some(tag) = tag_to_copy {
                    //     sio.output(0).add_tag(0, tag);
                    // }
                }
            }
            None => {
                if self.counter > 0 {
                    if num_samples_in_output >= self.pad_to {
                        let o = sio.output(0).slice::<T>();
                        o[..self.pad_to].fill(T::default());
                        produced += self.pad_to;
                        self.counter -= 1;
                    }
                } else {
                    let start_of_next_burst = sio.input(0).tags().iter().find_map(|x| match x {
                        ItemTag {
                            index,
                            tag: Tag::NamedUsize(n, len),
                        } => {
                            if *index == 0 && n == "burst_start" {
                                self.current_burst_len = Some(*len);
                                self.burst_partially_transmitted = false;
                                Some(0)
                            } else {
                                Some(*index)
                            }
                        }
                        _ => None,
                    });
                    if start_of_next_burst == Some(0) {
                        io.call_again = true;
                        return Ok(());
                    }
                    let i = sio.input(0).slice::<T>();
                    let mut n = num_samples_in_input;
                    if let Some(start_of_next_burst) = start_of_next_burst {
                        n = n.min(start_of_next_burst);
                    }
                    if !self.drop_untagged {
                        let o = sio.output(0).slice::<T>();
                        n = n.min(num_samples_in_output);
                        if n > 0 {
                            o[..n].copy_from_slice(&i[..n]);
                        }
                        produced += n;
                    }
                    consumed += n;
                }
            }
        }

        if consumed > 0 {
            debug!("consumed {consumed} samples");
            sio.input(0).consume(consumed);
        }
        if produced > 0 {
            debug!("produced {produced} samples");
            sio.output(0).produce(produced);
        }

        if sio.input(0).finished() && consumed == num_samples_in_input {
            io.finished = true;
        } else if consumed < num_samples_in_input && produced < num_samples_in_output {
            io.call_again = match self.current_burst_len {
                Some(remaining_samples) => {
                    if remaining_samples < num_samples_in_input - consumed {
                        true
                    } else if num_samples_in_input - consumed >= self.pad_to {
                        true
                    } else {
                        false
                    }
                }
                None => true,
            };
            if self.counter > 0 {
                io.call_again = true;
            }
        }
        if consumed == 0 && produced == 0 {
            debug!(
                "consumed and produced nothing. state: num_samples_in_input {num_samples_in_input}, num_samples_in_output {num_samples_in_output}, self.current_burst_len {:?}, self.partially_transmitted {}, self.pad_to {}, io.call_again {}",
                self.current_burst_len,
                self.burst_partially_transmitted,
                self.pad_to,
                io.call_again
            );
        }

        Ok(())
    }
}

use crate::prelude::*;

#[derive(Debug)]
enum StateTracker {
    HaveBurst(usize, usize, bool), // burst can be transmitted in chunks as long as there is enough space in out buffer, will start to transmit as many buffers as the output can fit
    // propagating the burst_start tag ensures the following blocks handle the samples as a unit, and the output buffer size requirement ensures the flowgraph does not stall on large bursts
    HaveUntaggedSamples(usize),
    Skip(usize),
    Pad(usize),
    ClaimPending,
}

/// Rewrite "burst_start" tags to a new value, padding with a supplied generator or discarding samples if the new burst size is larger or smaller.
///
/// # Stream Inputs
///
/// `in`: Input, expected to contain "burst_start" named_usize tags
///
/// # Stream Outputs
///
/// `out`: Output, potentially padded copy of the input samples
///
/// # Usage
/// ```rust
/// use futuresdr::blocks::{BurstPad, BurstSplit, BurstSizeRewriter};
/// // use futuresdr::blocks::seify::Builder;
/// use futuresdr::prelude::*;
/// /// produces a single tagged burst of 8 zeroes
/// #[derive(Block)]
/// struct DummyBurstSource<
///     O = DefaultCpuWriter<Complex32>,
/// >
/// where
///     O: CpuBufferWriter<Item = Complex32>,
/// {
///     #[output]
///     output: O,
/// }
/// impl<O> DummyBurstSource<O>
/// where
///     O: CpuBufferWriter<Item = Complex32>,
/// {
///     pub fn new() -> Self {
///         let mut out = O::default();
///         out.set_min_items(8);
///         Self{
///             output: out,
///         }
///     }
/// }
/// impl<O> Kernel for DummyBurstSource<O>
/// where
///     O: CpuBufferWriter<Item = Complex32>,
/// {
///     async fn work(
///         &mut self,
///         io: &mut WorkIo,
///         _m: &mut MessageOutputs,
///         _b: &mut BlockMeta,
///     ) -> Result<()> {
///         let (out, mut out_tags) = self.output.slice_with_tags();
///         if out.len() >= 8 {
///             out[..8].fill(Complex32::new(0.0, 0.0));
///             out_tags.add_tag(0, Tag::NamedUsize("burst_start".to_string(), 8));
///             self.output.produce(8);
///             io.finished = true;
///         }
///         Ok(())
///     }
/// }
/// // example pipeline to enable burst transmissions with the HackRF One in Half-Duplex mode:
/// // first, extend each burst for individual lossless transmission, as the hackrf might discard queued samples when switching back to receive mode
/// // then, pad the bursts to multiples of the hackrf transmit buffer size to ensure immediate transmission without the sink waiting to fill the buffer first
/// // finally, break long bursts up into sub-bursts no longer than the hackrf transmit buffer size, as longer burst transmissions are currently not supported by the driver.
/// // Overall, this ensures immediate and lossless transmission of bursts. While there is a chance for buffer underflow corrupting frames due to the chunking in the last step, this is very small, as the previous padding and a sufficiently sized buffer at the sink ensure that the required amount of samples to finish the original burst shoud be pretty much immediately available.
/// fn main() -> Result<()> {
///     // get a sample stream that contains "burst_start" tags
///     let source: DummyBurstSource = DummyBurstSource::new();
///     // keep track of the maximum expected burst size
///     let mut max_burst_size: usize = 8;
///     // pad each individual burst (there is only one in this example) to ensure lossless transmission with the HackRF One in Half-Duplex mode
///     let pad_head: BurstPad = BurstPad::new_for_hackrf();
///     // keep track of the maximum expected burst size
///     max_burst_size = pad_head.propagate_max_burst_size(max_burst_size);
///     // pad each burst by a given function over its original length, in this case to a multiple of the HackRF transmit buffer size
///     let pad: BurstSizeRewriter = BurstSizeRewriter::new_for_hackrf();
///     // keep track of the maximum expected burst size
///     max_burst_size = pad.propagate_max_burst_size(max_burst_size);
///     // split each burst into mutliple sub-bursts, here specifically for the HackRF One, which can only transmit bursts with a fixed/maximum size
///     let split_bursts: BurstSplit = BurstSplit::new_for_hackrf();
///     // keep track of the maximum expected burst size
///     max_burst_size = split_bursts.propagate_max_burst_size(max_burst_size);
///     // create sink with sufficiently sized input buffer to not stall on large bursts
///     // let sink = Builder::new("driver=soapy,soapy_driver=hackrf")?
///     //        .min_in_buffer_size(max_burst_size) // make sure the sink won't deadlock on large bursts due to insufficient buffer size
///     //        .build_sink()?;
///     // [connect and run Flowgraph]
///     Ok(())
/// }
/// ```
#[derive(Block)]
pub struct BurstSizeRewriter<
    T: Copy + Send + Default + 'static = Complex32,
    I = DefaultCpuReader<T>,
    O = DefaultCpuWriter<T>,
> where
    I: CpuBufferReader<Item = T>,
    O: CpuBufferWriter<Item = T>,
{
    #[input]
    input: I,
    #[output]
    output: O,
    state: StateTracker,
    burst_size_function: fn(usize) -> usize,
    default_value_generator: Box<dyn FnMut() -> T + Send + 'static>,
    _type: std::marker::PhantomData<T>,
}

impl<T: Copy + Send + Default + 'static, I, O> BurstSizeRewriter<T, I, O>
where
    I: CpuBufferReader<Item = T>,
    O: CpuBufferWriter<Item = T>,
{
    /// Create [`struct@BurstSizeRewriter`] block
    pub fn new(
        burst_size_function: fn(usize) -> usize,
        default_value_generator: impl FnMut() -> T + Send + 'static,
    ) -> Self {
        BurstSizeRewriter::<T, I, O> {
            input: I::default(),
            output: O::default(),
            state: StateTracker::ClaimPending,
            burst_size_function,
            default_value_generator: Box::new(default_value_generator),
            _type: std::marker::PhantomData,
        }
    }

    /// compute the maximum produced burst length based on the maximum expected input sample burst.
    /// useful for correctly sizing the input buffer of a burst-aware downstream sink to avoid deadlocks, as it needs to wait for the whole burst to be available before starting to precess it.
    pub fn propagate_max_burst_size(&self, max_input_burst_size: usize) -> usize {
        (self.burst_size_function)(max_input_burst_size)
    }
}

#[doc(hidden)]
impl<T: Copy + Send + Default + 'static, I, O> Kernel for BurstSizeRewriter<T, I, O>
where
    I: CpuBufferReader<Item = T>,
    O: CpuBufferWriter<Item = T>,
{
    async fn work(
        &mut self,
        io: &mut WorkIo,
        _m: &mut MessageOutputs,
        _b: &mut BlockMeta,
    ) -> Result<()> {
        let mut consumed = 0;
        let mut produced = 0;

        let (i, i_tags) = self.input.slice_with_tags();
        let (o, mut o_tags) = self.output.slice_with_tags();
        let num_samples_in_input = i.len();
        let num_slots_in_output = o.len();

        'outer: loop {
            match &self.state {
                StateTracker::HaveBurst(input_len_remaining, output_len_remaining, tag_pending) => {
                    let n_consume = (num_samples_in_input - consumed).min(*input_len_remaining);
                    let n_produce = (num_slots_in_output - produced).min(*output_len_remaining);
                    let n_copy = n_consume.min(n_produce);
                    if n_copy > 0 {
                        if *tag_pending {
                            // tag burst with modified length
                            o_tags.add_tag(
                                produced,
                                Tag::NamedUsize("burst_start".to_string(), *output_len_remaining),
                            );
                        }
                        o[produced..produced + n_copy]
                            .copy_from_slice(&i[consumed..consumed + n_copy]);
                        consumed += n_copy;
                        produced += n_copy;
                        if n_copy == *input_len_remaining && n_copy == *output_len_remaining {
                            self.state = StateTracker::ClaimPending;
                        } else if n_copy == *input_len_remaining {
                            self.state = StateTracker::Pad(*output_len_remaining - n_copy);
                        } else if n_copy == *output_len_remaining {
                            self.state = StateTracker::Skip(*input_len_remaining - n_copy);
                        } else {
                            self.state = StateTracker::HaveBurst(
                                *input_len_remaining - n_copy,
                                *output_len_remaining - n_copy,
                                false,
                            );
                        }
                    } else {
                        break 'outer;
                    }
                }
                StateTracker::HaveUntaggedSamples(num_remaining) => {
                    self.state = StateTracker::HaveBurst(*num_remaining, *num_remaining, false);
                    // TODO this just copies
                }
                StateTracker::Skip(num_samples) => {
                    let n_consume = (num_samples_in_input - consumed).min(*num_samples);
                    if n_consume > 0 {
                        consumed += n_consume;
                        if n_consume == *num_samples {
                            self.state = StateTracker::ClaimPending;
                        } else {
                            self.state = StateTracker::Skip(*num_samples - n_consume);
                        }
                    } else {
                        break 'outer;
                    }
                }
                StateTracker::Pad(num_samples) => {
                    let n_produce = (num_slots_in_output - produced).min(*num_samples);
                    if n_produce > 0 {
                        for slot in o.iter_mut().skip(produced).take(n_produce) {
                            *slot = (self.default_value_generator)();
                        }
                        produced += n_produce;
                        if n_produce == *num_samples {
                            self.state = StateTracker::ClaimPending;
                        } else {
                            self.state = StateTracker::Pad(*num_samples - n_produce);
                        }
                    } else {
                        break 'outer;
                    }
                }
                StateTracker::ClaimPending => {
                    // get new state depending on available input
                    if num_samples_in_input - consumed == 0 {
                        break 'outer;
                    } else {
                        self.state = i_tags
                            .iter()
                            .find_map(|x| match x {
                                ItemTag {
                                    index,
                                    tag: Tag::NamedUsize(n, len),
                                } => {
                                    if n == "burst_start" {
                                        if *index < consumed {
                                            warn!("dropping missed Tag: Tag::NamedUsize({n}, {len}) @ index {index}.");
                                            None
                                        } else if *index == consumed {
                                            // TODO index might be offset by consumed
                                            Some(StateTracker::HaveBurst(
                                                *len,
                                                (self.burst_size_function)(*len),
                                                true,
                                            ))
                                        } else {
                                            Some(StateTracker::HaveUntaggedSamples(
                                                *index - consumed,
                                            ))
                                        }
                                    } else {
                                        None
                                    }
                                }
                                _ => None,
                            })
                            .unwrap_or(StateTracker::HaveUntaggedSamples(
                                num_samples_in_input - consumed,
                            ));
                    }
                }
            }
        }

        if consumed > 0 {
            // debug!("consumed {consumed} samples");
            self.input.consume(consumed);
        }
        if produced > 0 {
            // debug!("produced {produced} samples");
            self.output.produce(produced);
        }

        if self.input.finished() && consumed == num_samples_in_input {
            io.finished = true;
        } else if self.input.finished() {
            warn!(
                "burst_size_rewriter block not yet terminated: state={:?} samples_left_in_input={} num_slots_left_in_output={}",
                self.state,
                num_samples_in_input - consumed,
                num_slots_in_output - produced
            );
        }

        // if consumed == 0 && produced == 0 {
        //     debug!(
        //         "consumed and produced nothing. state: num_samples_in_input {num_samples_in_input}, num_samples_in_output {num_slots_in_output}, io.call_again {}",
        //         io.call_again
        //     );
        // }

        Ok(())
    }
}

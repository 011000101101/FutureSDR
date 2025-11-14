use crate::runtime::BlockMeta;
use crate::runtime::BlockMetaBuilder;
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

// const HACKRF_BLOCK_SIZE: usize = 12288;
const HACKRF_BLOCK_SIZE: usize = 131072;

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
/// `out`: Output, potentially padded copy the input
///
/// # Usage
/// TODO
pub struct BurstSizeRewriter<T: core::marker::Copy + Send + Default + 'static> {
    state: StateTracker,
    burst_size_function: fn(usize) -> usize,
    default_value_generator: Box<dyn FnMut() -> T + Send + 'static>,
    _type: std::marker::PhantomData<T>,
}

impl<T: core::marker::Copy + Send + Default + 'static> BurstSizeRewriter<T> {
    /// Create [`struct@BurstSizeRewriter`] block
    pub fn new(
        burst_size_function: fn(usize) -> usize,
        default_value_generator: impl FnMut() -> T + Send + 'static,
    ) -> TypedBlock<Self> {
        TypedBlock::new(
            BlockMetaBuilder::new("BurstSizeRewriter").build(),
            StreamIoBuilder::new()
                .add_input::<T>("in")
                .add_output_with_size_relative_to_max_input::<T>(
                    "out",
                    move |input_sizes: &[usize]| {
                        let input_burst_size = *input_sizes.iter().max().unwrap();
                        burst_size_function(input_burst_size / size_of::<T>()) * size_of::<T>()
                    },
                )
                .build(),
            MessageIoBuilder::<Self>::new().build(),
            BurstSizeRewriter::<T> {
                state: StateTracker::ClaimPending,
                burst_size_function,
                default_value_generator: Box::new(default_value_generator),
                _type: std::marker::PhantomData,
            },
        )
    }
}

impl BurstSizeRewriter<Complex32> {
    /// Create [`struct@BurstSizeRewriter`] block specifically to pad sample streams for the HackRF One to multiples of the transmit buffer, to ensure immediate and unbroken transmission of bursts
    pub fn new_for_hackrf() -> TypedBlock<Self> {
        let burst_size_function = |input_size_max_items: usize| {
            ((input_size_max_items as f64 / HACKRF_BLOCK_SIZE as f64).ceil() as usize).max(1)
                * HACKRF_BLOCK_SIZE
        };
        let default_value_generator = || Complex32::default();
        BurstSizeRewriter::<Complex32>::new(burst_size_function, default_value_generator)
    }
}

#[doc(hidden)]
#[async_trait]
impl<T: core::marker::Copy + Send + Default + 'static> Kernel for BurstSizeRewriter<T> {
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

        'outer: loop {
            match &self.state {
                StateTracker::HaveBurst(input_len_remaining, output_len_remaining, tag_pending) => {
                    let n_consume = (num_samples_in_input - consumed).min(*input_len_remaining);
                    let n_produce = (num_slots_in_output - produced).min(*output_len_remaining);
                    let n_copy = n_consume.min(n_produce);
                    if n_copy > 0 {
                        if *tag_pending {
                            // tag burst with modified length
                            sio.output(0).add_tag(
                                produced,
                                Tag::NamedUsize("burst_start".to_string(), *output_len_remaining),
                            );
                        }
                        let i = sio.input(0).slice::<T>();
                        let o = sio.output(0).slice::<T>();
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
                        let o = sio.output(0).slice::<T>();
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
                                            Some(StateTracker::HaveBurst(
                                                *len,
                                                (self.burst_size_function)(*len),
                                                true,
                                            ))
                                        } else {
                                            Some(StateTracker::HaveUntaggedSamples(*index))
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
            sio.input(0).consume(consumed);
        }
        if produced > 0 {
            // debug!("produced {produced} samples");
            sio.output(0).produce(produced);
        }

        if sio.input(0).finished() && consumed == num_samples_in_input {
            io.finished = true;
        } else if sio.input(0).finished() {
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

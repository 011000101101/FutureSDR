use rand::Rng;

use futures::AsyncReadExt;

use crate::runtime::BlockMeta;
use crate::runtime::BlockMetaBuilder;
use crate::runtime::Kernel;
use crate::runtime::MessageIo;
use crate::runtime::MessageIoBuilder;
use crate::runtime::Result;
use crate::runtime::StreamIo;
use crate::runtime::StreamIoBuilder;
use crate::runtime::TypedBlock;
use crate::runtime::WorkIo;

/// Read samples from a file.
///
/// Samples are assumed to be encoded in the native format for the runtime. For
/// example, on most machines, that means little endian. For complex samples,
/// the real component must come before the imaginary component.
///
/// # Inputs
///
/// No inputs.
///
/// # Outputs
///
/// `out`: Output samples
///
/// # Usage
/// ```no_run
/// use futuresdr::blocks::FileSource;
/// use futuresdr::runtime::Flowgraph;
/// use num_complex::Complex;
///
/// let mut fg = Flowgraph::new();
///
/// // Loads 8-byte samples from the file
/// let source = fg.add_block(FileSource::<Complex<f32>>::new("my_filename.cf32", false));
/// ```
#[cfg_attr(docsrs, doc(cfg(not(target_arch = "wasm32"))))]
pub struct FileSource<T: Send + 'static> {
    file_name: String,
    file: Option<async_fs::File>,
    repeat: bool,
    repeat_count: Option<usize>,
    sto_count: Option<usize>,
    remaining_sto: usize,
    _type: std::marker::PhantomData<T>,
}

impl<T: Send + 'static> FileSource<T> {
    /// Create FileSource block
    pub fn new<S: Into<String>>(file_name: S, repeat: bool) -> TypedBlock<Self> {
        TypedBlock::new(
            BlockMetaBuilder::new("FileSource").build(),
            StreamIoBuilder::new().add_output::<T>("out").build(),
            MessageIoBuilder::new().build(),
            FileSource::<T> {
                file_name: file_name.into(),
                file: None,
                repeat,
                repeat_count: None,
                sto_count: None,
                remaining_sto: 0,
                _type: std::marker::PhantomData,
            },
        )
    }

    /// Create FileSource block with specific number of repeats
    pub fn new_with_repeat_count<S: Into<String>>(
        file_name: S,
        repeat_count: usize,
        samples_per_symbol: usize,
    ) -> TypedBlock<Self> {
        TypedBlock::new(
            BlockMetaBuilder::new("FileSource").build(),
            StreamIoBuilder::new().add_output::<T>("out").build(),
            MessageIoBuilder::new().build(),
            FileSource::<T> {
                file_name: file_name.into(),
                file: None,
                repeat: true,
                repeat_count: Some(repeat_count),
                sto_count: Some(samples_per_symbol),
                // sto_count: None,
                remaining_sto: 0,
                _type: std::marker::PhantomData,
            },
        )
    }
}

#[doc(hidden)]
#[async_trait]
impl<T: Send + 'static> Kernel for FileSource<T> {
    async fn work(
        &mut self,
        io: &mut WorkIo,
        sio: &mut StreamIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
    ) -> Result<()> {
        let out = sio.output(0).slice_unchecked::<u8>();
        let item_size = std::mem::size_of::<T>();

        debug_assert_eq!(out.len() % item_size, 0);

        let mut i = 0;

        while i < out.len() {
            if self.remaining_sto > 0 {
                let sto_now =
                    ((self.remaining_sto * item_size).min(out.len() - i) / item_size) * item_size;
                out[i..i + sto_now].fill(0);
                i += sto_now;
                self.remaining_sto -= sto_now / item_size;
                if self.remaining_sto > 0 {
                    break;
                }
            }
            match self.file.as_mut().unwrap().read(&mut out[i..]).await {
                Ok(0) => {
                    if self.repeat {
                        if let Some(remaining) = self.repeat_count {
                            if remaining == 0 {
                                io.finished = true;
                                break;
                            } else {
                                self.repeat_count = Some(remaining - 1);
                            }
                        }
                        self.file =
                            Some(async_fs::File::open(self.file_name.clone()).await.unwrap());
                        if let Some(sto_max) = self.sto_count {
                            let mut rng = rand::rng();
                            // generate a random number of 0s as integer and fractional offset [0, 1[ symbol
                            self.remaining_sto = rng.random_range(0..sto_max);
                        }
                    } else {
                        io.finished = true;
                        break;
                    }
                }
                Ok(written) => {
                    i += written;
                }
                Err(e) => panic!("FileSource: Error reading from file: {e:?}"),
            }
        }

        sio.output(0).produce(i / item_size);

        Ok(())
    }

    async fn init(
        &mut self,
        _sio: &mut StreamIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
    ) -> Result<()> {
        self.file = Some(async_fs::File::open(self.file_name.clone()).await.unwrap());
        if let Some(repeat_count) = self.repeat_count {
            debug_assert!(repeat_count > 0);
            self.repeat_count = Some(repeat_count - 1);
        }
        Ok(())
    }
}

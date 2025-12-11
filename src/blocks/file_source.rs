use crate::prelude::*;
use futuredsp::num_traits::Zero;
use rand::Rng;
use std::path::Path;
use std::path::PathBuf;

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
#[derive(Block)]
pub struct FileSource<T: Send + 'static, O: CpuBufferWriter<Item = T> = DefaultCpuWriter<T>> {
    file_path: PathBuf,
    file: Option<async_fs::File>,
    repeat: bool,
    repeat_count: Option<usize>,
    sto_count: Option<usize>,
    remaining_sto: usize,
    #[output]
    output: O,
}

impl<T: Send + 'static, O: CpuBufferWriter<Item = T>> FileSource<T, O> {
    /// Create FileSource block
    pub fn new(file_path: impl AsRef<Path>, repeat: bool) -> Self {
        Self {
            file_path: file_path.as_ref().to_path_buf(),
            file: None,
            repeat,
            repeat_count: None,
            sto_count: None,
            remaining_sto: 0,
            output: O::default(),
        }
    }

    /// Create FileSource block with specific number of repeats
    pub fn new_with_repeat_count<S: Into<String>>(
        file_path: impl AsRef<Path>,
        repeat_count: usize,
        samples_per_symbol: usize,
    ) -> Self {
        Self {
            file_path: file_path.as_ref().to_path_buf(),
            file: None,
            repeat: true,
            repeat_count: Some(repeat_count),
            sto_count: Some(samples_per_symbol),
            // sto_count: None,
            remaining_sto: 0,
            output: O::default(),
        }
    }
}

#[doc(hidden)]
impl<T: Send + Zero + Copy + 'static, O: CpuBufferWriter<Item = T>> Kernel for FileSource<T, O> {
    async fn work(
        &mut self,
        io: &mut WorkIo,
        _mio: &mut MessageOutputs,
        _meta: &mut BlockMeta,
    ) -> Result<()> {
        let out = self.output.slice();

        let out_bytes = unsafe {
            std::slice::from_raw_parts_mut(out.as_ptr() as *mut u8, std::mem::size_of_val(out))
        };

        let item_size = std::mem::size_of::<T>();
        let mut i = 0;

        while i < out_bytes.len() {
            if self.remaining_sto > 0 {
                let sto_now =
                    ((self.remaining_sto * item_size).min(out.len() - i) / item_size) * item_size;
                out[i..i + sto_now].fill(T::zero());
                i += sto_now;
                self.remaining_sto -= sto_now / item_size;
                if self.remaining_sto > 0 {
                    break;
                }
            }
            match self.file.as_mut().unwrap().read(&mut out_bytes[i..]).await {
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
                            Some(async_fs::File::open(self.file_path.clone()).await.unwrap());
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

        self.output.produce(i / item_size);

        Ok(())
    }

    async fn init(&mut self, _mio: &mut MessageOutputs, _meta: &mut BlockMeta) -> Result<()> {
        self.file = Some(async_fs::File::open(self.file_path.clone()).await.unwrap());
        if let Some(repeat_count) = self.repeat_count {
            debug_assert!(repeat_count > 0);
            self.repeat_count = Some(repeat_count - 1);
        }
        Ok(())
    }
}

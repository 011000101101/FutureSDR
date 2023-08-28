use seify::Args;
use seify::Device;
use seify::DeviceTrait;
use seify::Direction::Tx;
use seify::GenericDevice;
use seify::TxStreamer;

use crate::anyhow::{Context, Result};
use crate::blocks::seify::Builder;
use crate::blocks::seify::Config;
use crate::num_complex::Complex32;
use crate::runtime::Block;
use crate::runtime::BlockMeta;
use crate::runtime::BlockMetaBuilder;
use crate::runtime::ItemTag;
use crate::runtime::Kernel;
use crate::runtime::MessageIo;
use crate::runtime::MessageIoBuilder;
use crate::runtime::Pmt;
use crate::runtime::StreamIo;
use crate::runtime::StreamIoBuilder;
use crate::runtime::Tag;
use crate::runtime::WorkIo;

use super::builder::BuilderType;

/// Seify Sink block
pub struct Sink<D: DeviceTrait + Clone> {
    channels: Vec<usize>,
    dev: Device<D>,
    streamer: Option<D::TxStreamer>,
    start_time: Option<i64>,
}

impl<D: DeviceTrait + Clone> Sink<D> {
    pub(super) fn new(dev: Device<D>, channels: Vec<usize>, start_time: Option<i64>) -> Block {
        assert!(!channels.is_empty());

        let mut siob = StreamIoBuilder::new();

        if channels.len() == 1 {
            siob = siob.add_input::<Complex32>("in");
        } else {
            for i in 0..channels.len() {
                siob = siob.add_input::<Complex32>(&format!("in{}", i + 1));
            }
        }
        Block::new(
            BlockMetaBuilder::new("Sink").blocking().build(),
            siob.build(),
            MessageIoBuilder::new()
                .add_input("freq", Self::freq_handler)
                .add_input("gain", Self::gain_handler)
                .add_input("sample_rate", Self::sample_rate_handler)
                .add_input("cmd", Self::cmd_handler)
                .add_input("freq_offset", Self::freq_offset_handler)
                .build(),
            Self {
                channels,
                dev,
                start_time,
                streamer: None,
            },
        )
    }

    #[message_handler]
    fn cmd_handler(
        &mut self,
        _io: &mut WorkIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
        p: Pmt,
    ) -> Result<Pmt> {
        let c: Config = p.try_into()?;
        c.apply(&self.dev, &self.channels, Tx)?;
        Ok(Pmt::Ok)
    }

    #[message_handler]
    fn freq_handler(
        &mut self,
        _io: &mut WorkIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
        p: Pmt,
    ) -> Result<Pmt> {
        for c in &self.channels {
            match &p {
                Pmt::F32(v) => self.dev.set_frequency(Tx, *c, *v as f64)?,
                Pmt::F64(v) => self.dev.set_frequency(Tx, *c, *v)?,
                Pmt::U32(v) => self.dev.set_frequency(Tx, *c, *v as f64)?,
                Pmt::U64(v) => self.dev.set_frequency(Tx, *c, *v as f64)?,
                _ => return Ok(Pmt::InvalidValue),
            };
        }
        Ok(Pmt::Ok)
    }

    #[message_handler]
    fn gain_handler(
        &mut self,
        _io: &mut WorkIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
        p: Pmt,
    ) -> Result<Pmt> {
        for c in &self.channels {
            match &p {
                Pmt::F32(v) => self.dev.set_gain(Tx, *c, *v as f64)?,
                Pmt::F64(v) => self.dev.set_gain(Tx, *c, *v)?,
                Pmt::U32(v) => self.dev.set_gain(Tx, *c, *v as f64)?,
                Pmt::U64(v) => self.dev.set_gain(Tx, *c, *v as f64)?,
                _ => return Ok(Pmt::InvalidValue),
            };
        }
        Ok(Pmt::Ok)
    }

    #[message_handler]
    fn sample_rate_handler(
        &mut self,
        _io: &mut WorkIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
        p: Pmt,
    ) -> Result<Pmt> {
        for c in &self.channels {
            match &p {
                Pmt::F32(v) => self.dev.set_sample_rate(Tx, *c, *v as f64)?,
                Pmt::F64(v) => self.dev.set_sample_rate(Tx, *c, *v)?,
                Pmt::U32(v) => self.dev.set_sample_rate(Tx, *c, *v as f64)?,
                Pmt::U64(v) => self.dev.set_sample_rate(Tx, *c, *v as f64)?,
                _ => return Ok(Pmt::InvalidValue),
            };
        }
        Ok(Pmt::Ok)
    }

    #[message_handler]
    fn freq_offset_handler(
        &mut self,
        _io: &mut WorkIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
        p: Pmt,
    ) -> Result<Pmt> {
        let mut args = Args::new();
        let offset = match &p {
            Pmt::F32(v) => *v as f64,
            Pmt::F64(v) => *v,
            Pmt::U32(v) => *v as f64,
            Pmt::U64(v) => *v as f64,
            _ => return Ok(Pmt::InvalidValue),
        };
        args.set("Offset", offset.to_string());
        for c in &self.channels {
            let f = self.dev.frequency(Tx, *c).unwrap();
            self.dev.set_frequency_with_args(Tx, *c, f, args.clone())?
        }
        Ok(Pmt::Ok)
    }
}

#[doc(hidden)]
#[async_trait]
impl<D: DeviceTrait + Clone> Kernel for Sink<D> {
    async fn work(
        &mut self,
        io: &mut WorkIo,
        sio: &mut StreamIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
    ) -> Result<()> {
        let bufs: Vec<&[Complex32]> = sio
            .inputs_mut()
            .iter_mut()
            .map(|b| b.slice::<Complex32>())
            .collect();

        let streamer = self.streamer.as_mut().unwrap();
        let n = bufs.iter().map(|b| b.len()).min().unwrap_or(0);
        if n == 0 {
            return Ok(());
        }

        let t = sio.input(0).tags().iter().find_map(|x| match x {
            ItemTag {
                index,
                tag: Tag::NamedUsize(n, len),
            } => {
                if *index == 0 && n == "burst_start" {
                    Some(*len)
                } else {
                    None
                }
            }
            _ => None,
        });

        io.finished = sio.inputs().iter().any(|x| x.finished());

        let consumed = if let Some(len) = t {
            // debug!("sending in burst mode");
            if n >= len {
                // send burst
                let bufs: Vec<&[Complex32]> = bufs.iter().map(|b| &b[0..len]).collect();
                let ret = streamer.write(&bufs, None, true, 2_000_000)?;
                debug_assert_eq!(ret, len);
                ret
            } else {
                debug!("sink [burst-mode]: waiting for more samples before sending.");
                // wait for more samples
                0
            }
        } else {
            // debug!("sending in non-burst mode");
            // send in non-burst mode
            let ret = streamer.write(&bufs, None, false, 2_000_000)?;
            if ret != n {
                io.call_again = true;
            }
            println!("consumed: {}", ret);
            ret
        };

        sio.inputs_mut()
            .iter_mut()
            .for_each(|i| i.consume(consumed));

        Ok(())
    }

    async fn init(
        &mut self,
        _sio: &mut StreamIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
    ) -> Result<()> {
        self.streamer = Some(self.dev.tx_streamer(&self.channels)?);
        self.streamer
            .as_mut()
            .context("no stream")?
            .activate_at(self.start_time)?;

        Ok(())
    }

    async fn deinit(
        &mut self,
        _sio: &mut StreamIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
    ) -> Result<()> {
        self.streamer.as_mut().context("no stream")?.deactivate()?;
        Ok(())
    }
}

/// Seify Sink builder
pub struct SinkBuilder;

impl SinkBuilder {
    /// Create Seify Sink builder
    pub fn new() -> Builder<GenericDevice> {
        Builder::new(BuilderType::Sink)
    }
}

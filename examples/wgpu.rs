use std::time::Instant;
use std::iter::repeat_with;

use futuresdr::anyhow::Result;
use futuresdr::blocks::VectorSink;
use futuresdr::blocks::VectorSinkBuilder;
use futuresdr::blocks::VectorSourceBuilder;
use futuresdr::blocks::Wgpu;
use futuresdr::runtime::buffer::wgpu;
use futuresdr::runtime::Flowgraph;
use futuresdr::runtime::Runtime;

fn main() -> Result<()> {

    let n_items = 3_000_000;
    let orig: Vec<f32> = repeat_with(rand::random::<f32>).take(n_items).collect();

    let mut fg = Flowgraph::new();

    let src = VectorSourceBuilder::<f32>::new(orig.clone()).build();
    let broker = async_io::block_on(wgpu::Broker::new());
    let wgpu = Wgpu::new(broker, 8192);
    let snk = VectorSinkBuilder::<f32>::new().build();

    let src = fg.add_block(src);
    let wgpu = fg.add_block(wgpu);
    let snk = fg.add_block(snk);

    fg.connect_stream_with_type(src, "out", wgpu, "in", wgpu::H2D::new())?;
    fg.connect_stream_with_type(wgpu, "out", snk, "in", wgpu::D2H::new())?;

    let now = Instant::now();
    fg = Runtime::new().run(fg)?;
    let duration = now.elapsed();
    println!("flowgraph took {}", duration.as_secs_f64());

    let snk = fg.block_async::<VectorSink<f32>>(snk).unwrap();
    let v = snk.items();

    assert_eq!(v.len(), n_items);
    for i in 0..v.len() {
        assert!((orig[i] * 12.0 - v[i]).abs() < f32::EPSILON);
    }

    Ok(())
}

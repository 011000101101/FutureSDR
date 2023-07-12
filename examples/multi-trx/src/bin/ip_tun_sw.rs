use std::collections::HashMap;
use std::thread::sleep;
use clap::Parser;
use std::time::Duration;
use async_process::Command;
use forky_tun::{self, Configuration};
// use futures::StreamExt;
// use futures::sink::SinkExt;
use std::net::Ipv4Addr;
use tokio;

use futuresdr::anyhow::Result;
use futuresdr::async_io;
use futuresdr::async_io::block_on;
use futuresdr::async_io::Timer;
use futuresdr::async_net::UdpSocket;
use futuresdr::blocks::Apply;
use futuresdr::blocks::Combine;
use futuresdr::blocks::Fft;
use futuresdr::blocks::FftDirection;
// use futuresdr::blocks::FirBuilder;
use futuresdr::blocks::MessagePipe;
use futuresdr::blocks::Selector;
use futuresdr::blocks::SelectorDropPolicy as DropPolicy;
use futuresdr::futures::channel::mpsc;
use futuresdr::futures::StreamExt;
use futuresdr::log::info;
use futuresdr::log::warn;
use futuresdr::num_complex::Complex32;
use futuresdr::runtime::buffer::circular::Circular;
use futuresdr::runtime::Flowgraph;
use futuresdr::runtime::Pmt;
use futuresdr::runtime::Runtime;

use multitrx::MessageSelector;

use multitrx::IPDSCPRewriter;
use multitrx::MetricsReporter;
use multitrx::TcpSink;
use multitrx::TcpSource;
use multitrx::Complex32Serializer;
use multitrx::Complex32Deserializer;
use multitrx::AWGNComplex32;

use wlan::MAX_PAYLOAD_SIZE;
use wlan::fft_tag_propagation as wlan_fft_tag_propagation;
use wlan::Decoder as WlanDecoder;
use wlan::Delay as WlanDelay;
// use wlan::Encoder as WlanEncoder;
use multitrx::Encoder as WlanEncoder;
use wlan::FrameEqualizer as WlanFrameEqualizer;
use wlan::Mac as WlanMac;
use wlan::Mapper as WlanMapper;
use wlan::Mcs as WlanMcs;
use wlan::MovingAverage as WlanMovingAverage;
use wlan::Prefix as WlanPrefix;
use wlan::SyncLong as WlanSyncLong;
use wlan::SyncShort as WlanSyncShort;
use wlan::MAX_SYM;

use zigbee::modulator as zigbee_modulator;
use zigbee::IqDelay as ZigbeeIqDelay;
// use zigbee::Mac as ZigbeeMac;
use multitrx::ZigbeeMac;
use zigbee::ClockRecoveryMm as ZigbeeClockRecoveryMm;
use zigbee::Decoder as ZigbeeDecoder;


const PAD_FRONT: usize = 10000;
const PAD_TAIL: usize = 10000;

#[derive(Parser, Debug)]
#[clap(version)]
struct Args {
    /// TX MCS
    #[clap(long, value_parser = WlanMcs::parse, default_value = "qpsk12")]
    wlan_mcs: WlanMcs,
    /// padding front and back
    #[clap(long, default_value_t = 10000)]
    wlan_pad_len: usize,
    /// local IP to bind to
    #[clap(long, value_parser, default_value = "0.0.0.0")]
    local_ip: String,
    /// remote IP to connect to
    #[clap(long, value_parser)]
    remote_ip: String,
    /// local IP to bind to
    #[clap(long, value_parser, default_value = "172.18.0.1:1340")]
    metrics_reporting_socket: String,
    /// local UDP port to receive messages to send
    #[clap(long, value_parser, default_value = "1341")]
    protocol_switching_ctrl_port: u32,
    /// send periodic messages for testing
    #[clap(long, value_parser)]
    tx_interval: Option<f32>,
    /// Stream Spectrum data at ws://0.0.0.0:9001
    #[clap(long, value_parser)]
    spectrum: bool,
    /// Drop policy to apply on the selector.
    #[clap(short, long, default_value = "none")]
    drop_policy: DropPolicy,
    /// Path to JSON mapping ports to ip dscp priority values to override specific flow priorities
    // #[clap(long, value_parser = parse_flow_priority_json, default_value = "")]
    #[clap(long, value_parser)]
    flow_priority_file: String,
    /// TCPExchanger local sink port
    #[clap(long, value_parser)]
    local_tcp_sink_port: u32,
    /// TCPExchanger remote sink socket address
    #[clap(long, value_parser)]
    remote_tcp_sink_address: String,
    /// Receive noise power
    #[clap(long, value_parser)]
    rx_noise_power: f32,
}

const DSCP_EF: u8 = 0b101110 << 2;
const NUM_PROTOCOLS: usize = 2;
static MTU_VALUES: [usize; NUM_PROTOCOLS] = [
    MAX_PAYLOAD_SIZE, // WiFi
    256 - 4 - 5 - 2  // Zigbee: 256 bytes max frame size - 4 bytes TUN metadata - 5 bytes mac header - 2 bytes mac footer (checksum)
];

fn main() -> Result<()> {
    let args = Args::parse();
    println!("Configuration: {:?}", args);

    let flow_priority_map: HashMap<u16, u8> = HashMap::from([
        (14550, DSCP_EF),
        (18570, DSCP_EF),
        (10317, DSCP_EF),  // https://gazebosim.org/api/transport/11.0/envvars.html
        (10318, DSCP_EF),
    ]);  // TODO

    let mut size = 4096;
    let prefix_in_size = loop {
        if size / 8 >= MAX_SYM * 64 {
            break size;
        }
        size += 4096
    };
    let mut size = 4096;
    let prefix_out_size = loop {
        if size / 8 >= PAD_FRONT + std::cmp::max(PAD_TAIL, 1) + 320 + MAX_SYM * 80 {
            break size;
        }
        size += 4096
    };

    let mut fg = Flowgraph::new();

    let tcp_sink = fg.add_block(TcpSink::new(args.local_tcp_sink_port));
    let tcp_source = fg.add_block(TcpSource::new(args.remote_tcp_sink_address));
    let iq_serializer = fg.add_block(Complex32Serializer::new());
    let iq_deserializer = fg.add_block(Complex32Deserializer::new());

    //Sink Selector
    let sink_selector = Selector::<Complex32, 2, 1>::new(args.drop_policy);
    let input_index_port_id = sink_selector
        .message_input_name_to_id("input_index")
        .expect("No input_index port found!");
    let sink_selector = fg.add_block(sink_selector);
    // fg.connect_stream(sink_selector, "out0", fir, "in")?;

    // fg.connect_stream(fir, "out", iq_serializer, "in")?;
    fg.connect_stream(sink_selector, "out0", iq_serializer, "in")?;
    fg.connect_stream(iq_serializer, "out", tcp_sink, "in")?;

    let rx_noise = fg.add_block(AWGNComplex32::new(args.rx_noise_power));
    //source selector
    let src_selector = Selector::<Complex32, 1, 2>::new(args.drop_policy);
    let output_index_port_id = src_selector
        .message_input_name_to_id("output_index")
        .expect("No output_index port found!");
    let src_selector = fg.add_block(src_selector);
    fg.connect_stream(tcp_source, "out", iq_deserializer, "in")?;
    fg.connect_stream(iq_deserializer, "out", rx_noise, "in")?;
    fg.connect_stream(rx_noise, "out", src_selector, "in0")?;

    // ============================================
    // WLAN TRANSMITTER
    // ============================================
    let wlan_mac = fg.add_block(WlanMac::new([0x42; 6], [0x23; 6], [0xff; 6]));
    let wlan_encoder = fg.add_block(WlanEncoder::new(args.wlan_mcs));
    fg.connect_message(wlan_mac, "tx", wlan_encoder, "tx")?;
    let wlan_mapper = fg.add_block(WlanMapper::new());
    fg.connect_stream(wlan_encoder, "out", wlan_mapper, "in")?;
    let mut wlan_fft = Fft::with_options(
        64,
        FftDirection::Inverse,
        true,
        Some((1.0f32 / 52.0).sqrt()),
    );
    wlan_fft.set_tag_propagation(Box::new(wlan_fft_tag_propagation));
    let wlan_fft = fg.add_block(wlan_fft);
    fg.connect_stream(wlan_mapper, "out", wlan_fft, "in")?;
    let wlan_prefix = fg.add_block(WlanPrefix::new(PAD_FRONT, PAD_TAIL));
    fg.connect_stream_with_type(
        wlan_fft,
        "out",
        wlan_prefix,
        "in",
        Circular::with_size(prefix_in_size),
    )?;
    
    fg.connect_stream_with_type(
        wlan_prefix,
        "out",
        sink_selector,
        "in0",
        Circular::with_size(prefix_out_size),
    )?;

    // ============================================
    // WLAN RECEIVER
    // ============================================

    let metrics_reporter = fg.add_block(MetricsReporter::new(args.metrics_reporting_socket, args.local_ip.clone()));
    
    let wlan_delay = fg.add_block(WlanDelay::<Complex32>::new(16));
    fg.connect_stream(src_selector, "out0", wlan_delay, "in")?;

    let wlan_complex_to_mag_2 = fg.add_block(Apply::new(|i: &Complex32| i.norm_sqr()));
    let wlan_float_avg = fg.add_block(WlanMovingAverage::<f32>::new(64));
    fg.connect_stream(src_selector, "out0", wlan_complex_to_mag_2, "in")?;
    fg.connect_stream(wlan_complex_to_mag_2, "out", wlan_float_avg, "in")?;

    let wlan_mult_conj = fg.add_block(Combine::new(|a: &Complex32, b: &Complex32| a * b.conj()));
    let wlan_complex_avg = fg.add_block(WlanMovingAverage::<Complex32>::new(48));
    fg.connect_stream(src_selector, "out0", wlan_mult_conj, "in0")?;
    fg.connect_stream(wlan_delay, "out", wlan_mult_conj, "in1")?;
    fg.connect_stream(wlan_mult_conj, "out", wlan_complex_avg, "in")?;

    let wlan_divide_mag = fg.add_block(Combine::new(|a: &Complex32, b: &f32| a.norm() / b));
    fg.connect_stream(wlan_complex_avg, "out", wlan_divide_mag, "in0")?;
    fg.connect_stream(wlan_float_avg, "out", wlan_divide_mag, "in1")?;

    let wlan_sync_short = fg.add_block(WlanSyncShort::new());
    fg.connect_stream(wlan_delay, "out", wlan_sync_short, "in_sig")?;
    fg.connect_stream(wlan_complex_avg, "out", wlan_sync_short, "in_abs")?;
    fg.connect_stream(wlan_divide_mag, "out", wlan_sync_short, "in_cor")?;

    let wlan_sync_long = fg.add_block(WlanSyncLong::new());
    fg.connect_stream(wlan_sync_short, "out", wlan_sync_long, "in")?;

    let mut wlan_fft = Fft::new(64);
    wlan_fft.set_tag_propagation(Box::new(wlan_fft_tag_propagation));
    let wlan_fft = fg.add_block(wlan_fft);
    fg.connect_stream(wlan_sync_long, "out", wlan_fft, "in")?;

    let wlan_frame_equalizer = fg.add_block(WlanFrameEqualizer::new());
    fg.connect_stream(wlan_fft, "out", wlan_frame_equalizer, "in")?;

    let wlan_decoder = fg.add_block(WlanDecoder::new());
    fg.connect_stream(wlan_frame_equalizer, "out", wlan_decoder, "in")?;

    let (wlan_rxed_sender, mut wlan_rxed_frames) = mpsc::channel::<Pmt>(100);
    let wlan_message_pipe = fg.add_block(MessagePipe::new(wlan_rxed_sender));
    fg.connect_message(wlan_decoder, "rx_frames", wlan_message_pipe, "in")?;
    let wlan_blob_to_udp = fg.add_block(futuresdr::blocks::BlobToUdp::new("127.0.0.1:55555"));
    fg.connect_message(wlan_decoder, "rx_frames", wlan_blob_to_udp, "in")?;
    fg.connect_message(wlan_decoder, "rx_frames", metrics_reporter, "rx_wifi_in")?;
    let wlan_blob_to_udp = fg.add_block(futuresdr::blocks::BlobToUdp::new("127.0.0.1:55556"));
    fg.connect_message(wlan_decoder, "rftap", wlan_blob_to_udp, "in")?;


    // ========================================
    // ZIGBEE RECEIVER
    // ========================================
    let mut last: Complex32 = Complex32::new(0.0, 0.0);
    let mut iir: f32 = 0.0;
    let alpha = 0.00016;
    let avg = fg.add_block(Apply::new(move |i: &Complex32| -> f32 {
        let phase = (last.conj() * i).arg();
        last = *i;
        iir = (1.0 - alpha) * iir + alpha * phase;
        phase - iir
    }));

    let omega = 2.0;
    let gain_omega = 0.000225;
    let mu = 0.5;
    let gain_mu = 0.03;
    let omega_relative_limit = 0.0002;
    let mm = fg.add_block(ZigbeeClockRecoveryMm::new(
        omega,
        gain_omega,
        mu,
        gain_mu,
        omega_relative_limit,
    ));

    let zigbee_decoder = fg.add_block(ZigbeeDecoder::new(6));
    let zigbee_mac = fg.add_block(ZigbeeMac::new());
    //let null_sink = fg.add_block(NullSink::<u8>::new());
    //let zigbee_blob_to_udp = fg.add_block(futuresdr::blocks::BlobToUdp::new("127.0.0.1:55557"));
    let (zigbee_rxed_sender, mut zigbee_rxed_frames) = mpsc::channel::<Pmt>(100);
    let zigbee_message_pipe = fg.add_block(MessagePipe::new(zigbee_rxed_sender));

    fg.connect_stream(src_selector, "out1", avg, "in")?;
    fg.connect_stream(avg, "out", mm, "in")?;
    fg.connect_stream(mm, "out", zigbee_decoder, "in")?;
    fg.connect_message(zigbee_decoder, "out", zigbee_mac, "rx")?;
    fg.connect_message(zigbee_mac, "rxed", zigbee_message_pipe, "in")?;
    fg.connect_message(zigbee_mac, "rxed", metrics_reporter, "rx_in")?;
    //fg.connect_stream(zigbee_mac, "out", null_sink, "in")?;
    //fg.connect_message(zigbee_mac, "out", zigbee_blob_to_udp, "in")?;


    // ========================================
    // ZIGBEE TRANSMITTER
    // ========================================

    //let zigbee_mac = fg.add_block(ZigbeeMac::new());
    let zigbee_modulator = fg.add_block(zigbee_modulator());
    let zigbee_iq_delay = fg.add_block(ZigbeeIqDelay::new());

    fg.connect_stream(zigbee_mac, "out", zigbee_modulator, "in")?;
    fg.connect_stream(zigbee_modulator, "out", zigbee_iq_delay, "in")?;
    fg.connect_stream(zigbee_iq_delay, "out", sink_selector, "in1")?;

    // ========================================
    // MESSAGE INPUT SELECTOR
    // ========================================

    // message input selector
    let message_selector = MessageSelector::new();
    let message_in_port_id = message_selector
        .message_input_name_to_id("message_in")
        .expect("No message_in port found!");
    let output_selector_port_id = message_selector
        .message_input_name_to_id("output_selector")
        .expect("No output_selector port found!");
    let message_selector = fg.add_block(message_selector);
    fg.connect_message(message_selector, "out0", wlan_mac, "tx")?;
    fg.connect_message(message_selector, "out1", zigbee_mac, "tx")?;

    // ========================================
    // FLOW PRIORITY TO IP DSCP MAPPER
    // ========================================

    let ip_dscp_rewriter = IPDSCPRewriter::new(flow_priority_map);
    let fg_tx_port = ip_dscp_rewriter
        .message_input_name_to_id("in")
        .expect("No message_in port found!");
    let ip_dscp_rewriter = fg.add_block(ip_dscp_rewriter);
    fg.connect_message(ip_dscp_rewriter, "out", metrics_reporter, "tx_in")?;
    fg.connect_message(ip_dscp_rewriter, "out", message_selector, "message_in")?;


    // ========================================
    // Spectrum
    // ========================================
    // if args.spectrum {
    //     let snk = fg.add_block(
    //         WebsocketSinkBuilder::<f32>::new(9001)
    //             .mode(WebsocketSinkMode::FixedDropping(2048))
    //             .build(),
    //     );
    //     let fft = fg.add_block(Fft::new(2048));
    //     let shift = fg.add_block(FftShift::new());
    //     let keep = fg.add_block(Keep1InN::new(0.5, 10));
    //     let cpy = fg.add_block(futuresdr::blocks::Copy::<Complex32>::new());

    //     fg.connect_stream(src, "out", cpy, "in")?;
    //     fg.connect_stream(cpy, "out", fft, "in")?;
    //     fg.connect_stream(fft, "out", shift, "in")?;
    //     fg.connect_stream(shift, "out", keep, "in")?;
    //     fg.connect_stream(keep, "out", snk, "in")?;
    // }

    let rt = Runtime::new();
    let (_fg, mut handle) = block_on(rt.start(fg));
    let mut input_handle = handle.clone();

    // if tx_interval is set, send messages periodically
    if let Some(tx_interval) = args.tx_interval {
        let mut seq = 0u64;
        let mut myhandle = handle.clone();
        rt.spawn_background(async move {
            loop {
                Timer::after(Duration::from_secs_f32(tx_interval)).await;
                myhandle
                    .call(
                        message_selector,
                        message_in_port_id,
                        Pmt::Blob(format!("FutureSDR {}", seq).as_bytes().to_vec()),
                    )
                    .await
                    .unwrap();
                seq += 1;
            }
        });
    }

    info!("Acting as IP tunnel from {} to {}.", args.local_ip.clone(), args.remote_ip);
    let mut tun_config = Configuration::default();
        tun_config
            .name("chanem")
            .address(args.local_ip.clone())
            .netmask((255, 255, 255, 0))
            .destination(args.remote_ip.clone())
            .queues(1)
            .mtu(MTU_VALUES[0].try_into().unwrap())
            .up();
        #[cfg(target_os = "linux")]
        tun_config.platform(|tun_config| {
            tun_config.packet_information(true);
        });

    let rt_tokio = tokio::runtime::Runtime::new().unwrap();
    let (tx_tun_dev1, mut rx_tun_dev1) = tokio::sync::mpsc::channel(1);
    let _keep_channel_open = tx_tun_dev1.clone();
    // let (tx_tun_dev2, mut rx_tun_dev2) = oneshot::channel::<forky_tun::AsyncQueue>();
    // let (tx_tun_dev3, mut rx_tun_dev3) = oneshot::channel::<forky_tun::AsyncQueue>();
    rt_tokio.spawn(async move {
        let tun_dev = forky_tun::create_as_async(&tun_config).unwrap();
        let mut tun_queues = tun_dev.queues().unwrap();
        let tun_queue1 = tun_queues.remove(0);
        // let tun_queue2 = tun_queues.remove(0);
        // let tun_queue3 = tun_queues.remove(0);
        // println!("{:?}", tun_queue2.get_ref().tun);
        match tx_tun_dev1.send(tun_queue1).await {
            Ok(_) => {},
            Err(_) => panic!("could not send TUN interface handle out of async creation context."),
        }
        // println!("{:?}", tun_queue2.get_ref().as_raw_fd());
        // tx_tun_dev2.send(tun_queue2);
        // tx_tun_dev3.send(tun_queue3);
        println!("TUN setup successful.");
    });

    println!("receiving TUN queue");
    let tun_queue1: std::sync::Arc<forky_tun::AsyncQueue> = std::sync::Arc::new(rx_tun_dev1.blocking_recv().unwrap());
    println!("received TUN queue");
    rx_tun_dev1.close();
    let tun_queue2 = tun_queue1.clone();
    let tun_queue3 = tun_queue1.clone();

    rt.spawn_background(async move {
        println!("initialized sender.");
        let mut buf = vec![0u8; 1024];
        loop {
            // println!("blub");
            match tun_queue1.recv(&mut buf).await {
                Ok(n) => {
                    // 4 bytes offset due to flag bytes added to the front of each packet by TUN interface
                    // if format!("{}.{}.{}.{}", buf[20], buf[21], buf[22], buf[23]) != remote_ip1 {
                    //     println!("{:?}", buf);
                    //     warn!("received packet with dst_ip not matching {}", remote_ip1);
                    //     continue;  // TODO
                    // }

                    print!("s");
                    handle
                    .call(
                        ip_dscp_rewriter,
                        fg_tx_port,
                        Pmt::Blob(buf[0..n].to_vec())
                    )
                    .await
                    .unwrap();
                    // if let Ok(_res) = socket_metrics.send(format!("{},tx,{}", local_ip1, n).as_bytes()).await {
                    //     // info!("server sent a frame.")
                    // } else {
                    //     warn!("could not send metric update.")
                    // }
                },
                Err(err) => panic!("Error: {:?}", err),
            }
        }
    });

    rt.spawn_background(async move {
        println!("initialized WiFi receiver.");
        loop {
            if let Some(p) = wlan_rxed_frames.next().await {
                if let Pmt::Blob(v) = p {
                    // info!("received frame, size {}", v.len() - 24);
                    print!("r");
                    tun_queue2.send(&v[24..].to_vec()).await.unwrap();
                    // if let Ok(_) = socket_metrics2.send(format!("{},rx", local_ip2).as_bytes()).await {
                    //     // info!("server received a frame.")
                    // } else {
                    //     warn!("could not send metric update.")
                    // }
                } else {
                    warn!("pmt to tx was not a blob");
                }
            } else {
                warn!("cannot read from MessagePipe receiver");
            }
        }
    });

    rt.spawn_background(async move {
        println!("initialized ZigBee receiver.");
        loop {
            if let Some(p) = zigbee_rxed_frames.next().await {
                if let Pmt::Blob(v) = p {
                    // info!("received Zigbee frame size {}", v.len());
                    print!("r");
                    tun_queue3.send(&v.to_vec()).await.unwrap();
                    // if let Ok(_) = socket_metrics3.send(format!("{},rx", local_ip3).as_bytes()).await {
                    //     // info!("server received a frame.")
                    // } else {
                    //     warn!("could not send metric update.")
                    // }
                } else {
                    warn!("pmt to tx was not a blob");
                }
            } else {
                warn!("cannot read from MessagePipe receiver");
           }

        }
    });

    // protocol switching message handler:
    info!("listening for protocol switch on port {}.", args.protocol_switching_ctrl_port);
    let socket = block_on(UdpSocket::bind((Ipv4Addr::UNSPECIFIED, args.protocol_switching_ctrl_port as u16))).unwrap();

    rt.spawn_background(async move {
        let mut current_mtu = MTU_VALUES[0];
        let mut buf = vec![0u8; 1024];
        loop {
            match socket.recv_from(&mut buf).await {
                Ok((n, s)) => {
                    let the_string = std::str::from_utf8(&buf[0..n]).expect("not UTF-8");
                    let new_protocol_index = the_string.trim_end().parse::<u32>().unwrap();
                    println!("received protocol number {} from {:?}", new_protocol_index, s);

                    if (new_protocol_index as usize) < NUM_PROTOCOLS {
                        let new_mtu = MTU_VALUES[new_protocol_index as usize];
                        if new_mtu < current_mtu {
                            // switch to smaller MTU before changing the PHY to avoid dropping packets
                            if let Err(e) = Command::new("sh").arg("-c").arg(format!("ifconfig chanem mtu {} up", new_mtu)).output().await {
                                warn!("could not change MTU of TUN interface. Original error message: {}", e);
                            };
                            current_mtu = new_mtu;
                        }
                        let new_index = new_protocol_index as u32;
                        println!("Setting source index to {}", new_index);
                        async_io::block_on(
                            input_handle
                                .call(
                                    src_selector,
                                    output_index_port_id,
                                    Pmt::U32(new_index)
                                )
                        ).unwrap();
                        async_io::block_on(
                            input_handle
                                .call(
                                    sink_selector,
                                    input_index_port_id,
                                    Pmt::U32(new_index)
                                )
                        ).unwrap();
                        async_io::block_on(
                            input_handle
                                .call(
                                    message_selector,
                                    output_selector_port_id,
                                    Pmt::U32(new_index)
                                )
                        ).unwrap();
                        if new_mtu > current_mtu {
                            // switch to larger MTU after changing the PHY to avoid dropping packets
                            if let Err(e) = Command::new("sh").arg("-c").arg(format!("ifconfig chanem mtu {} up", new_mtu)).output().await {
                                warn!("could not change MTU of TUN interface. Original error message: {}", e);
                            };
                            current_mtu = new_mtu;
                        }
                    }
                    else {
                        println!("Invalid protocol index.")
                    }
                }
                Err(e) => println!("ERROR: {:?}", e),
            }
        }
    });

    println!("running in background, disabling manual protocol selection.");
    loop {
        sleep(Duration::from_secs(5));
    }

}

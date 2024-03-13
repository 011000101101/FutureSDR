use futuresdr::anyhow::Result;
use futuresdr::macros::async_trait;
use std::collections::HashMap;
use std::f32::consts::PI;

use futuresdr::futures::channel::mpsc;

use futuresdr::log::{info, warn};
use futuresdr::macros::message_handler;
use futuresdr::num_complex::Complex32;
use futuresdr::runtime::Block;
use futuresdr::runtime::BlockMeta;
use futuresdr::runtime::BlockMetaBuilder;
use futuresdr::runtime::Kernel;
use futuresdr::runtime::MessageIo;
use futuresdr::runtime::MessageIoBuilder;
use futuresdr::runtime::Pmt;
use futuresdr::runtime::StreamIo;
use futuresdr::runtime::StreamIoBuilder;
use futuresdr::runtime::Tag;
use futuresdr::runtime::WorkIo;

use crate::utilities::*;

use rustfft::{FftDirection, FftPlanner};

// impl Copy for usize {}
// impl Copy for isize {}
// impl Copy for u8 {}
// impl Copy for u16 {}
// impl Copy for u32 {}
// impl Copy for i32 {}
// impl Copy for f32 {}
// impl Copy for f64 {}
// impl Copy for bool {}

#[derive(Debug, Copy, Clone, PartialEq)]
enum DecoderState {
    Detect,
    Sync,
    SfoCompensation,
}
#[repr(usize)]
#[derive(Debug, Copy, Clone, PartialEq)]
enum SyncState {
    NetId1 = 0,
    NetId2 = 1,
    Downchirp1 = 2,
    Downchirp2 = 3,
    QuarterDown = 4,
    Synced(usize),
}
impl From<usize> for SyncState {
    fn from(orig: usize) -> Self {
        match orig {
            0_usize => SyncState::NetId1,
            1_usize => SyncState::NetId2,
            2_usize => SyncState::Downchirp1,
            3_usize => SyncState::Downchirp2,
            4_usize => SyncState::QuarterDown,
            _ => {
                // warn!("implicit conversion from usize to SyncState::SYNCED(usize)");  // TODO
                SyncState::Synced(orig)
            }
        }
    }
}
impl From<SyncState> for usize {
    fn from(orig: SyncState) -> Self {
        match orig {
            SyncState::NetId1 => 0_usize,
            SyncState::NetId2 => 1_usize,
            SyncState::Downchirp1 => 2_usize,
            SyncState::Downchirp2 => 3_usize,
            SyncState::QuarterDown => 4_usize,
            SyncState::Synced(value) => value,
        }
    }
}

pub struct FrameSync {
    m_state: DecoderState, //< Current state of the synchronization
    m_center_freq: u32,    //< RF center frequency
    m_bw: u32,             //< Bandwidth
    // m_samp_rate: u32,               //< Sampling rate
    m_sf: usize, //< Spreading factor
    // m_cr: u8,                       //< Coding rate
    // m_pay_len: u32,                 //< payload length
    // m_has_crc: u8,                  //< CRC presence
    // m_invalid_header: u8,           //< invalid header checksum
    m_impl_head: bool,  //< use implicit header mode
    m_os_factor: usize, //< oversampling factor
    // m_sync_words: Vec<usize>, //< vector containing the two sync words (network identifiers)
    // m_ldro: bool,                        //< use of low datarate optimisation mode
    m_n_up_req: SyncState, //< number of consecutive upchirps required to trigger a detection

    m_number_of_bins: usize,     //< Number of bins in each lora Symbol
    m_samples_per_symbol: usize, //< Number of samples received per lora symbols
    m_symb_numb: usize,          //<number of payload lora symbols
    m_received_head: bool, //< indicate that the header has be decoded and received by this block
    // m_noise_est: f64,            //< estimate of the noise
    in_down: Vec<Complex32>,     //< downsampled input
    m_downchirp: Vec<Complex32>, //< Reference downchirp
    m_upchirp: Vec<Complex32>,   //< Reference upchirp

    frame_cnt: usize,       //< Number of frame received
    symbol_cnt: SyncState,  //< Number of symbols already received
    bin_idx: Option<usize>, //< value of previous lora symbol
    // bin_idx_new: i32, //< value of newly demodulated symbol
    m_preamb_len: usize,        //< Number of consecutive upchirps in preamble
    additional_upchirps: usize, //< indicate the number of additional upchirps found in preamble (in addition to the minimum required to trigger a detection)

    // cx_in: Vec<Complex32>,  //<input of the FFT
    // cx_out: Vec<Complex32>, //<output of the FFT

    // one_symbol_off: i32, //< indicate that we are offset by one symbol after the preamble  // local to general_work
    additional_symbol_samp: Vec<Complex32>, //< save the value of the last 1.25 downchirp as it might contain the first payload symbol
    preamble_raw: Vec<Complex32>, //<vector containing the preamble upchirps without any synchronization
    preamble_raw_up: Vec<Complex32>, //<vector containing the upsampled preamble upchirps without any synchronization
    // downchirp_raw: Vec<Complex32>,    //< vetor containing the preamble downchirps without any synchronization
    preamble_upchirps: Vec<Complex32>, //<vector containing the preamble upchirps
    net_id_samp: Vec<Complex32>,       //< vector of the oversampled network identifier samples
    // net_ids: Vec<Option<usize>>,       //< values of the network identifiers received
    up_symb_to_use: usize, //< number of upchirp symbols to use for CFO and STO frac estimation
    k_hat: usize,          //< integer part of CFO+STO
    preamb_up_vals: Vec<usize>, //< value of the preamble upchirps

    m_cfo_frac: f64, //< fractional part of CFO
    // m_cfo_frac_bernier: f32, //< fractional part of CFO using Berniers algo
    // m_cfo_int: i32,                               //< integer part of CFO
    m_sto_frac: f32,                 //< fractional part of CFO
    sfo_hat: f32,                    //< estimated sampling frequency offset
    sfo_cum: f32,                    //< cumulation of the sfo
    cfo_frac_sto_frac_est: bool, //< indicate that the estimation of CFO_frac and STO_frac has been performed
    cfo_frac_correc: Vec<Complex32>, //< cfo frac correction vector
    // cfo_sfo_frac_correc: Vec<Complex32>, //< correction vector accounting for cfo and sfo
    // symb_corr: Vec<Complex32>, //< symbol with CFO frac corrected
    down_val: Option<usize>, //< value of the preamble downchirps
    // net_id_off: i32,                    //< offset of the network identifier
    // m_should_log: bool, //< indicate that the sync values should be logged
    // off_by_one_id: f32, //< Indicate that the network identifiers where off by one and corrected (float used as saved in a float32 bin file)
    tag_from_msg_handler_to_work_channel: (mpsc::Sender<Pmt>, mpsc::Receiver<Pmt>),
    known_valid_net_ids: [[bool; 256]; 256],
    known_valid_net_ids_reverse: [[bool; 256]; 256],
    net_id: [u16; 2],
    ready_to_detect: bool,
}

impl FrameSync {
    pub fn new(
        center_freq: u32,
        bandwidth: u32,
        sf: usize,
        impl_head: bool,
        sync_word: Vec<usize>,
        os_factor: usize,
        preamble_len: Option<usize>,
    ) -> Block {
        let preamble_len_tmp = preamble_len.unwrap_or(8);
        if preamble_len_tmp < 5 {
            panic!("Preamble length should be greater than 5!"); // only warning in original implementation
        }
        // let sync_word_tmp: Vec<usize> = if sync_word.len() == 1 {
        //     let tmp = sync_word[0];
        //     vec![((tmp & 0xF0_usize) >> 4) << 3, (tmp & 0x0F_usize) << 3]
        // } else {
        //     sync_word
        // };
        let m_number_of_bins_tmp = 1_usize << sf;
        let m_samples_per_symbol_tmp = m_number_of_bins_tmp * os_factor;
        let (m_upchirp_tmp, m_downchirp_tmp) = build_ref_chirps(sf, 1); // vec![0; m_number_of_bins_tmp]
                                                                        // let (m_upchirp_tmp, m_downchirp_tmp) = build_ref_chirps(sf, os_factor); // TODO

        let mut known_valid_net_ids: [[bool; 256]; 256] = [[false; 256]; 256];
        known_valid_net_ids[sync_word_tmp[0]][sync_word_tmp[1]] = true;
        let mut known_valid_net_ids_reverse: [[bool; 256]; 256] = [[false; 256]; 256];
        known_valid_net_ids_reverse[sync_word_tmp[1]][sync_word_tmp[0]] = true;

        Block::new(
            BlockMetaBuilder::new("FrameSync").build(),
            StreamIoBuilder::new()
                .add_input::<Complex32>("in")
                .add_output::<Complex32>("out")
                .add_output::<f32>("log_out")
                .build(),
            MessageIoBuilder::new()
                .add_input("frame_info", Self::frame_info_handler)
                .add_input("payload_crc_result", Self::payload_crc_result_handler)
                // .add_input("noise_est", Self::noise_est_handler)
                .add_output("snr")
                .add_output("frame_detected")
                .build(),
            FrameSync {
                m_state: DecoderState::Detect, //< Current state of the synchronization
                m_center_freq: center_freq,    //< RF center frequency
                m_bw: bandwidth,               //< Bandwidth
                m_sf: sf,                      //< Spreading factor

                // m_sync_words: sync_word_tmp, //< vector containing the two sync words (network identifiers)
                m_os_factor: os_factor, //< oversampling factor

                m_preamb_len: preamble_len_tmp, //< Number of consecutive upchirps in preamble
                // net_ids: vec![None; 2],         //< values of the network identifiers received
                m_n_up_req: From::<usize>::from(preamble_len_tmp - 3), //< number of consecutive upchirps required to trigger a detection
                up_symb_to_use: preamble_len_tmp - 4, //< number of upchirp symbols to use for CFO and STO frac estimation

                m_sto_frac: 0.0, //< fractional part of CFO

                m_impl_head: impl_head, //< use implicit header mode

                m_number_of_bins: m_number_of_bins_tmp, //< Number of bins in each lora Symbol
                m_samples_per_symbol: m_samples_per_symbol_tmp, //< Number of samples received per lora symbols
                additional_symbol_samp: vec![Complex32::new(0., 0.); 2 * m_samples_per_symbol_tmp], //< save the value of the last 1.25 downchirp as it might contain the first payload symbol
                m_upchirp: m_upchirp_tmp,     //< Reference upchirp
                m_downchirp: m_downchirp_tmp, //< Reference downchirp
                preamble_upchirps: vec![
                    Complex32::new(0., 0.);
                    preamble_len_tmp * m_number_of_bins_tmp
                ], //<vector containing the preamble upchirps
                preamble_raw_up: vec![
                    Complex32::new(0., 0.);
                    (preamble_len_tmp + 3) * m_number_of_bins_tmp
                ], //<vector containing the upsampled preamble upchirps without any synchronization
                cfo_frac_correc: vec![Complex32::new(0., 0.); m_number_of_bins_tmp], //< cfo frac correction vector
                // cfo_sfo_frac_correc: vec![Complex32::new(0., 0.); m_number_of_bins_tmp], //< correction vector accounting for cfo and sfo
                // symb_corr: vec![Complex32::new(0., 0.); m_number_of_bins_tmp], //< symbol with CFO frac corrected
                in_down: vec![Complex32::new(0., 0.); m_number_of_bins_tmp], //< downsampled input
                preamble_raw: vec![Complex32::new(0., 0.); m_number_of_bins_tmp * preamble_len_tmp], //<vector containing the preamble upchirps without any synchronization
                net_id_samp: vec![
                    Complex32::new(0., 0.);
                    (m_samples_per_symbol_tmp as f32 * 2.5) as usize
                ], //< vector of the oversampled network identifier samples

                bin_idx: None,                 //< value of previous lora symbol
                symbol_cnt: SyncState::NetId2, //< Number of symbols already received
                k_hat: 0,                      //< integer part of CFO+STO
                preamb_up_vals: vec![0; preamble_len_tmp - 3], //< value of the preamble upchirps
                frame_cnt: 0,                  //< Number of frame received

                // cx_in: vec![Complex32::new(0., 0.); m_number_of_bins_tmp], //<input of the FFT
                // cx_out: vec![Complex32::new(0., 0.); m_number_of_bins_tmp], //<output of the FFT

                // m_samp_rate: u32,               //< Sampling rate  // unused
                // m_cr: u8,                       //< Coding rate  // local to frame info handler
                // m_pay_len: u32,                 //< payload length  // local to frame info handler
                // m_has_crc: u8,                  //< CRC presence  // local to frame info handler
                // m_invalid_header: u8,           //< invalid header checksum  // local to frame info handler
                // m_ldro: bool,                        //< use of low datarate optimisation mode  // local to frame info handler
                m_symb_numb: 0,         //<number of payload lora symbols
                m_received_head: false, //< indicate that the header has be decoded and received by this block
                // m_noise_est: f64,            //< estimate of the noise  // local to noise est handler

                // bin_idx_new: i32, //< value of newly demodulated symbol  // local to work
                additional_upchirps: 0, //< indicate the number of additional upchirps found in preamble (in addition to the minimum required to trigger a detection)

                // one_symbol_off: i32, //< indicate that we are offset by one symbol after the preamble  // local to work
                // downchirp_raw: Vec<Complex32>,    //< vetor containing the preamble downchirps without any synchronization  // unused
                m_cfo_frac: 0.0, //< fractional part of CFO
                // m_cfo_frac_bernier: 0.0, //< fractional part of CFO using Berniers algo
                // m_cfo_int: i32,                               //< integer part of CFO  // local to work
                sfo_hat: 0.0,                 //< estimated sampling frequency offset
                sfo_cum: 0.0,                 //< cumulation of the sfo
                cfo_frac_sto_frac_est: false, //< indicate that the estimation of CFO_frac and STO_frac has been performed

                down_val: None, //< value of the preamble downchirps
                // net_id_off: i32,                    //< offset of the network identifier  // local to work
                // m_should_log: false, //< indicate that the sync values should be logged
                // off_by_one_id: f32  // local to work
                tag_from_msg_handler_to_work_channel: mpsc::channel::<Pmt>(1),
                known_valid_net_ids: known_valid_net_ids,
                known_valid_net_ids_reverse: known_valid_net_ids_reverse,
                net_id: [0; 2],
                ready_to_detect: true,
            },
        )
    }

    fn my_roundf(number: f32) -> isize {
        if number > 0.0 {
            (number + 0.5) as isize
        } else {
            (number - 0.5).ceil() as isize
        }
    }

    // fn forecast(int noutput_items, gr_vector_int &ninput_items_required)
    //     {
    //         ninput_items_required[0] = (m_os_factor * (m_number_of_bins + 2));
    //     }

    // fn estimate_cfo_frac(&self, samples: &[Complex32]) -> (Vec<Complex32>, f64) {
    //     // create longer downchirp
    //     let mut downchirp_aug: Vec<Complex32> =
    //         vec![Complex32::new(0., 0.); self.up_symb_to_use * self.m_number_of_bins];
    //     for i in 0_usize..self.up_symb_to_use {
    //         downchirp_aug[(i * self.m_number_of_bins)..((i + 1) * self.m_number_of_bins)]
    //             .copy_from_slice(&self.m_downchirp[0..self.m_number_of_bins]);
    //     }
    //
    //     // Dechirping
    //     let dechirped: Vec<Complex32> = volk_32fc_x2_multiply_32fc(&samples, &downchirp_aug);
    //     // prepare FFT
    //     // zero padded
    //     // let mut cx_in_cfo: Vec<Complex32> = vec![Complex32::new(0., 0.), 2 * self.up_symb_to_use * self.m_number_of_bins];
    //     // cx_in_cfo[..(self.up_symb_to_use * self.m_number_of_bins)].copy_from_slice(dechirped.as_slice());
    //     let mut cx_out_cfo: Vec<Complex32> =
    //         vec![Complex32::new(0., 0.); 2 * self.up_symb_to_use * self.m_number_of_bins];
    //     cx_out_cfo[..(self.up_symb_to_use * self.m_number_of_bins)]
    //         .copy_from_slice(dechirped.as_slice());
    //     // do the FFT
    //     FftPlanner::new()
    //         .plan_fft(cx_out_cfo.len(), FftDirection::Forward)
    //         .process(&mut cx_out_cfo);
    //     // Get magnitude
    //     let fft_mag_sq: Vec<f32> = volk_32fc_magnitude_squared_32f(&cx_out_cfo);
    //     // get argmax here
    //     let k0: usize = argmax_float(&fft_mag_sq);
    //
    //     // get three spectral lines
    //     let y_1 = fft_mag_sq[(k0 - 1) % (2 * self.up_symb_to_use * self.m_number_of_bins)] as f64;
    //     let y0 = fft_mag_sq[k0] as f64;
    //     let y1 = fft_mag_sq[(k0 + 1) % (2 * self.up_symb_to_use * self.m_number_of_bins)] as f64;
    //     // set constant coeff
    //     let u = 64. * self.m_number_of_bins as f64 / 406.5506497; // from Cui yang (15)
    //     let v = u * 2.4674;
    //     // RCTSL
    //     let wa = (y1 - y_1) / (u * (y1 + y_1) + v * y0);
    //     let ka = wa * self.m_number_of_bins as f64 / core::f64::consts::PI;
    //     let k_residual = ((k0 as f64 + ka) / 2. / self.up_symb_to_use as f64) % 1.;
    //     let cfo_frac = k_residual - if k_residual > 0.5 { 1. } else { 0. };
    //     // Correct CFO frac in preamble
    //     let cfo_frac_correc_aug: Vec<Complex32> = (0_usize
    //         ..self.up_symb_to_use * self.m_number_of_bins)
    //         .map(|x| {
    //             Complex32::from_polar(
    //                 1.,
    //                 -2. * PI * (cfo_frac as f32) / self.m_number_of_bins as f32 * x as f32,
    //             )
    //         })
    //         .collect();
    //
    //     let preamble_upchirps = volk_32fc_x2_multiply_32fc(&samples, &cfo_frac_correc_aug);
    //
    //     (preamble_upchirps, cfo_frac)
    // }

    fn estimate_cfo_frac_bernier(&self, samples: &[Complex32]) -> (Vec<Complex32>, f64) {
        let mut fft_val: Vec<Complex32> =
            vec![Complex32::new(0., 0.); self.up_symb_to_use * self.m_number_of_bins];
        let mut k0: Vec<usize> = vec![0; self.up_symb_to_use];
        let mut k0_mag: Vec<f64> = vec![0.; self.up_symb_to_use];
        for i in 0_usize..self.up_symb_to_use {
            // info!(
            //     "samples: {}, self.m_downchirp: {}",
            //     samples.len(),
            //     self.m_downchirp.len()
            // );
            // Dechirping
            let dechirped: Vec<Complex32> = volk_32fc_x2_multiply_32fc(
                &samples[(i * self.m_number_of_bins)..((i + 1) * self.m_number_of_bins)],
                &self.m_downchirp,
            );
            let mut cx_out_cfo: Vec<Complex32> = dechirped;
            // info!("dechirped: {}", cx_out_cfo.len());
            // do the FFT
            FftPlanner::new()
                .plan_fft(cx_out_cfo.len(), FftDirection::Forward)
                .process(&mut cx_out_cfo);
            let fft_mag_sq: Vec<f32> = volk_32fc_magnitude_squared_32f(&cx_out_cfo);
            // info!(
            //     "i = {}, range 1 [{}-{}], range 2 [{}-{}]",
            //     i,
            //     (i * self.m_number_of_bins),
            //     ((i + 1) * self.m_number_of_bins),
            //     0_usize,
            //     self.m_number_of_bins,
            // );
            // info!(
            //     "fft_val: {}, cx_out_cfo: {}",
            //     fft_val.len(),
            //     cx_out_cfo.len()
            // );
            fft_val[(i * self.m_number_of_bins)..((i + 1) * self.m_number_of_bins)]
                .copy_from_slice(&cx_out_cfo[0_usize..self.m_number_of_bins]);
            // Get magnitude
            // get argmax here
            k0[i] = argmax_float(&fft_mag_sq);

            k0_mag[i] = fft_mag_sq[k0[i]] as f64;
        }
        // get argmax
        let idx_max: usize = argmax_float(&k0_mag);
        let mut four_cum = Complex32::new(0., 0.);
        for i in 0_usize..(self.up_symb_to_use - 1) {
            four_cum += fft_val[idx_max + self.m_number_of_bins * i]
                * (fft_val[idx_max + self.m_number_of_bins * (i + 1)]).conj();
        }
        let cfo_frac = -four_cum.arg() as f64 / 2. / std::f64::consts::PI;
        // Correct CFO in preamble
        let cfo_frac_correc_aug: Vec<Complex32> = (0_usize
            ..(self.up_symb_to_use * self.m_number_of_bins))
            .map(|x| {
                Complex32::from_polar(
                    1.,
                    -2. * PI * cfo_frac as f32 / self.m_number_of_bins as f32 * x as f32,
                )
            })
            .collect();
        let preamble_upchirps = volk_32fc_x2_multiply_32fc(samples, &cfo_frac_correc_aug);
        (preamble_upchirps, cfo_frac)
    }

    fn estimate_sto_frac(&self) -> f32 {
        // int k0;
        // double Y_1, Y0, Y1, u, v, ka, wa, k_residual;
        // float sto_frac = 0;

        // std::vector<gr_complex> dechirped(m_number_of_bins);
        // kiss_fft_cpx *cx_in_sto = new kiss_fft_cpx[2 * m_number_of_bins];
        // kiss_fft_cpx *cx_out_sto = new kiss_fft_cpx[2 * m_number_of_bins];

        // std::vector<float> fft_mag_sq(2 * m_number_of_bins);
        // for (size_t i = 0; i < 2 * m_number_of_bins; i++)
        // {
        //     fft_mag_sq[i] = 0;
        // }
        // kiss_fft_cfg cfg_sto = kiss_fft_alloc(2 * m_number_of_bins, 0, 0, 0);

        let mut fft_mag_sq: Vec<f32> = vec![0.; 2 * self.m_number_of_bins];
        for i in 0_usize..self.up_symb_to_use {
            // Dechirping
            let dechirped: Vec<Complex32> = volk_32fc_x2_multiply_32fc(
                &self.preamble_upchirps
                    [(self.m_number_of_bins * i)..(self.m_number_of_bins * (i + 1))],
                &self.m_downchirp,
            );

            let mut cx_out_sto: Vec<Complex32> =
                vec![Complex32::new(0., 0.); 2 * self.m_number_of_bins];
            cx_out_sto[..self.m_number_of_bins].copy_from_slice(&dechirped);
            // do the FFT
            FftPlanner::new()
                .plan_fft(cx_out_sto.len(), FftDirection::Forward)
                .process(&mut cx_out_sto);
            // Get magnitude

            fft_mag_sq = volk_32fc_magnitude_squared_32f(&cx_out_sto)
                .iter()
                .zip(fft_mag_sq.iter())
                .map(|(x, y)| x + y)
                .collect();
        }

        // get argmax here
        let k0 = argmax_float(&fft_mag_sq);

        // get three spectral lines
        let y_1 = fft_mag_sq[my_modulo(k0 as isize - 1, 2 * self.m_number_of_bins)] as f64;
        let y0 = fft_mag_sq[k0] as f64;
        let y1 = fft_mag_sq[(k0 + 1) % (2 * self.m_number_of_bins)] as f64;

        // set constant coeff
        let u = 64. * self.m_number_of_bins as f64 / 406.5506497; // from Cui yang (eq.15)
        let v = u * 2.4674;
        // RCTSL
        let wa = (y1 - y_1) / (u * (y1 + y_1) + v * y0);
        let ka = wa * self.m_number_of_bins as f64 / std::f64::consts::PI;
        let k_residual = ((k0 as f64 + ka) / 2.) % (1.);

        (k_residual - if k_residual > 0.5 { 1. } else { 0. }) as f32
    }

    fn get_symbol_val(samples: &[Complex32], ref_chirp: &[Complex32]) -> Option<usize> {
        // double sig_en = 0;
        // std::vector<float> fft_mag(m_number_of_bins);
        // volk::vector<gr_complex> dechirped(m_number_of_bins);

        // kiss_fft_cfg cfg = kiss_fft_alloc(m_number_of_bins, 0, 0, 0);

        // Multiply with ideal downchirp
        let dechirped = volk_32fc_x2_multiply_32fc(samples, ref_chirp);

        let mut cx_out: Vec<Complex32> = dechirped;
        // do the FFT
        FftPlanner::new()
            .plan_fft(cx_out.len(), FftDirection::Forward)
            .process(&mut cx_out);

        // Get magnitude
        let fft_mag = volk_32fc_magnitude_squared_32f(&cx_out);
        //     sig_en += fft_mag[i];
        // }
        let sig_en: f64 = fft_mag.iter().map(|x| *x as f64).fold(0., |acc, e| acc + e);
        // Return argmax here

        if sig_en != 0. {
            Some(argmax_float(&fft_mag))
        } else {
            None
        }
    }

    // fn determine_energy(&self, samples: &Vec<Complex32>, length: Option<usize>) -> f32 {
    //     let length_tmp = length.unwrap_or(1);
    //     let magsq_chirp = volk_32fc_magnitude_squared_32f(
    //         &samples[0_usize..(self.m_number_of_bins * length_tmp)],
    //     );
    //     let energy_chirp = magsq_chirp.iter().fold(0., |acc, e| acc + e);
    //     return energy_chirp / self.m_number_of_bins as f32 / length_tmp as f32;
    // }

    fn determine_snr(&self, samples: &[Complex32]) -> f32 {
        // double tot_en = 0;
        // std::vector<float> fft_mag(m_number_of_bins);
        // std::vector<gr_complex> dechirped(m_number_of_bins);

        // let mut cx_out: Vec<Complex32> = samples.iter().map(|x| *x).collect();
        // FftPlanner::new()
        //     .plan_fft(cx_out.len(), FftDirection::Forward)
        //     .process(&mut cx_out);
        // let asdf = volk_32fc_magnitude_squared_32f(&cx_out);
        // let broad_energy = asdf.iter().map(|x| *x as f64).fold(0., |acc, e| acc + e);
        // println!("broadband energy vec: {:?}", asdf);
        // println!("broadband energy: {}", broad_energy);

        // Multiply with ideal downchirp
        let dechirped = volk_32fc_x2_multiply_32fc(samples, &self.m_downchirp);

        let mut cx_out: Vec<Complex32> = dechirped;
        // do the FFT
        FftPlanner::new()
            .plan_fft(cx_out.len(), FftDirection::Forward)
            .process(&mut cx_out);

        // Get magnitude
        let fft_mag = volk_32fc_magnitude_squared_32f(&cx_out);
        //     sig_en += fft_mag[i];
        // }
        // Return argmax here
        let max_idx = argmax_float(&fft_mag);
        // println!("{:?}", fft_mag);
        // println!("{max_idx}");
        let sig_en = fft_mag[max_idx] as f64;
        // let sig_en_samples = 33;
        // let noise_en_samples = self.m_number_of_bins / 2;
        let noise_en = fft_mag.iter().map(|x| *x as f64).fold(0., |acc, e| acc + e);
        // let noise_en_narrow_est = noise_en / fft_mag.len() as f64;

        // let noise_idx = (max_idx + self.m_number_of_bins / 2) % self.m_number_of_bins;
        // let mut noise_en_narrow = fft_mag
        //     .iter()
        //     .cycle()
        //     .skip(my_modulo(
        //         noise_idx as isize - (noise_en_samples as isize / 2),
        //         self.m_number_of_bins,
        //     ))
        //     .take(noise_en_samples)
        //     .map(|x| *x as f64)
        //     .fold(0., |acc, e| acc + e)
        //     / noise_en_samples as f64;
        // let sig_plus_noise_en = fft_mag
        //     .iter()
        //     .cycle()
        //     .skip(my_modulo(
        //         max_idx as isize - (sig_en_samples as isize / 2),
        //         self.m_number_of_bins,
        //     ))
        //     .take(sig_en_samples)
        //     .map(|x| *x as f64)
        //     .fold(0., |acc, e| acc + e);
        // println!(
        //     "SNR_alt {}dB",
        //     (10. * (sig_plus_noise_en / (noise_en - sig_plus_noise_en)).log10())
        // );
        // let sig_en = sig_plus_noise_en - noise_en_narrow * sig_en_samples as f64;
        // // let sig_en = sig_plus_noise_en;
        // // let sig_en = fft_mag.iter().map(|x| *x as f64).fold(0., |acc, e| acc + e);
        // let noise_en = noise_en_narrow * fft_mag.len() as f64;
        // assert!(fft_mag.len() == self.m_number_of_bins);
        // println!("{sig_en}");
        // println!("{noise_en}");
        // println!("{}");
        // let sig_en = sig_plus_noise_en - noise_en_narrow_est * sig_en_samples as f64;
        let noise_en = noise_en - sig_en;
        let dechirped_snr = 10. * (sig_en / (noise_en)).log10();
        // let dechirped_snr = (10. * (sig_en / noise_en).log10());
        // println!("{dechirped_snr}dB");
        // println!("coding_gain {}dB", coding_gain);
        // println!("SNR_ {}dB", dechirped_snr);
        dechirped_snr as f32
    }

    // self.m_noise_est only referenced here, so a noop?
    // #[message_handler]
    // fn noise_est_handler(
    //     &mut self,
    //     _io: &mut WorkIo,
    //     mio: &mut MessageIo<Self>,
    //     _meta: &mut BlockMeta,
    //     p: Pmt,
    // ) -> Result<Pmt> {
    //     if let Pmt::F32(noise_est) = p {
    //         self.m_noise_est = p;
    //     } else {
    //         warn!("noise_est pmt was not an f32");
    //     }
    //     Ok(Pmt::Null)
    // }

    #[message_handler]
    fn payload_crc_result_handler(
        &mut self,
        _io: &mut WorkIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
        p: Pmt,
    ) -> Result<Pmt> {
        if let Pmt::Bool(crc_valid) = p {
            // payload decoded successfully, cache current net_ids for future frame corrections
            if crc_valid
                && !self.known_valid_net_ids[self.net_id[0] as usize][self.net_id[1] as usize]
            {
                info!(
                    "payload decoded successfully, caching new net id: [{}, {}]",
                    self.net_id[0], self.net_id[1]
                );
                self.known_valid_net_ids[self.net_id[0] as usize][self.net_id[1] as usize] = true; // TODO
                self.known_valid_net_ids_reverse[self.net_id[1] as usize]
                    [self.net_id[0] as usize] = true;
            } else if !crc_valid {
                info!(
                    "failed to decode payload for netid [{}, {}], dropping.",
                    self.net_id[0], self.net_id[1]
                );
            }
            self.ready_to_detect = true;
        } else {
            warn!("payload_crc_result pmt was not a Bool");
        }
        Ok(Pmt::Null)
    }

    #[message_handler]
    fn frame_info_handler(
        &mut self,
        _io: &mut WorkIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
        p: Pmt,
    ) -> Result<Pmt> {
        if let Pmt::MapStrPmt(mut frame_info) = p {
            let m_cr: usize = if let Pmt::Usize(temp) = frame_info.get("cr").unwrap() {
                *temp
            } else {
                panic!("invalid cr")
            };
            let m_pay_len: usize = if let Pmt::Usize(temp) = frame_info.get("pay_len").unwrap() {
                *temp
            } else {
                panic!("invalid pay_len")
            };
            let m_has_crc: bool = if let Pmt::Bool(temp) = frame_info.get("crc").unwrap() {
                *temp
            } else {
                panic!("invalid m_has_crc")
            };
            // uint8_t
            let ldro_mode_tmp: LdroMode =
                if let Pmt::Bool(temp) = frame_info.get("ldro_mode").unwrap() {
                    if *temp {
                        LdroMode::ENABLE
                    } else {
                        LdroMode::DISABLE
                    }
                } else {
                    panic!("invalid ldro_mode")
                };
            let m_invalid_header = if let Pmt::Bool(temp) = frame_info.get("err").unwrap() {
                *temp
            } else {
                panic!("invalid err flag")
            };

            // info!("FrameSync: received header info");

            if m_invalid_header {
                self.reset()
            } else {
                if m_has_crc {
                    // TODO blocking until crc result is in to avoid interleaving caching decision with next frame (with new unchecked net ids)
                    self.ready_to_detect = false;
                }
                let m_ldro: LdroMode = if ldro_mode_tmp == LdroMode::AUTO {
                    if (1_usize << self.m_sf) as f32 * 1e3 / self.m_bw as f32 > LDRO_MAX_DURATION_MS
                    {
                        LdroMode::ENABLE
                    } else {
                        LdroMode::DISABLE
                    }
                } else {
                    ldro_mode_tmp
                };

                let m_symb_numb_tmp = 8_isize
                    + ((2 * m_pay_len as isize - self.m_sf as isize
                        + 2
                        + (!self.m_impl_head) as isize * 5
                        + if m_has_crc { 4 } else { 0 }) as f64
                        / (self.m_sf - 2 * m_ldro as usize) as f64)
                        .ceil() as isize
                        * (4 + m_cr as isize);
                assert!(m_symb_numb_tmp >= 0, "Döner: computed negative symbol number");
                self.m_symb_numb = m_symb_numb_tmp as usize;
                self.m_received_head = true;
                frame_info.insert(String::from("is_header"), Pmt::Bool(false));
                frame_info.insert(String::from("symb_numb"), Pmt::Usize(self.m_symb_numb));
                frame_info.remove("ldro_mode");
                frame_info.insert(String::from("ldro"), Pmt::Bool(m_ldro as usize != 0));
                let frame_info_pmt = Pmt::MapStrPmt(frame_info);
                self.tag_from_msg_handler_to_work_channel
                    .0
                    .try_send(frame_info_pmt)
                    .unwrap();
            }
        } else {
            warn!("noise_est pmt was not a Map/Dict");
        }
        Ok(Pmt::Null)
    }

    // fn set_sf(&mut self, sf: usize) {
    //     self.m_sf = sf;
    //     self.m_number_of_bins = 1 << self.m_sf;
    //     self.m_samples_per_symbol = self.m_number_of_bins * self.m_os_factor;
    //     self.additional_symbol_samp
    //         .resize(2 * self.m_samples_per_symbol, Complex32::new(0., 0.));
    //     self.m_upchirp
    //         .resize(self.m_number_of_bins, Complex32::new(0., 0.));
    //     self.m_downchirp
    //         .resize(self.m_number_of_bins, Complex32::new(0., 0.));
    //     self.preamble_upchirps.resize(
    //         self.m_preamb_len * self.m_number_of_bins,
    //         Complex32::new(0., 0.),
    //     );
    //     self.preamble_raw_up.resize(
    //         (self.m_preamb_len + 3) * self.m_samples_per_symbol,
    //         Complex32::new(0., 0.),
    //     );
    //     self.cfo_frac_correc
    //         .resize(self.m_number_of_bins, Complex32::new(0., 0.));
    //     self.cfo_sfo_frac_correc
    //         .resize(self.m_number_of_bins, Complex32::new(0., 0.));
    //     self.symb_corr
    //         .resize(self.m_number_of_bins, Complex32::new(0., 0.));
    //     self.in_down
    //         .resize(self.m_number_of_bins, Complex32::new(0., 0.));
    //     self.preamble_raw.resize(
    //         self.m_preamb_len * self.m_number_of_bins,
    //         Complex32::new(0., 0.),
    //     );
    //     self.net_id_samp.resize(
    //         (self.m_samples_per_symbol as f32 * 2.5) as usize,
    //         Complex32::new(0., 0.),
    //     ); // we should be able to move up to one quarter of symbol in each direction
    //     let (upchirp_tmp, downchirp_tmp) = build_ref_chirps(self.m_sf, 1);
    //     // let (upchirp_tmp, downchirp_tmp) = build_ref_chirps(self.m_sf, self.m_os_factor);  // TODO
    //     self.m_upchirp = upchirp_tmp;
    //     self.m_downchirp = downchirp_tmp;
    //
    //     // Constrain the noutput_items argument passed to forecast and general_work.
    //     // set_output_multiple causes the scheduler to ensure that the noutput_items argument passed to forecast and general_work will be an integer multiple of
    //     // https://www.gnuradio.org/doc/doxygen/classgr_1_1block.html#a63d67fd758b70c6f2d7b7d4edcec53b3
    //     // set_output_multiple(m_number_of_bins);  // unnecessary, noutput_items is only used for the noutput_items < m_number_of_bins check, which is equal noutput_items / self.m_number_of_bins < 1
    // }

    fn detect(&mut self, input: &[Complex32]) -> (isize, usize) {
        let bin_idx_new_opt = FrameSync::get_symbol_val(&self.in_down, &self.m_downchirp);

        let condition = if let Some(bin_idx_new) = bin_idx_new_opt {
            if ((((bin_idx_new as i32 - self.bin_idx.map(|x| x as i32).unwrap_or(-1)).abs() + 1)
                % self.m_number_of_bins as i32)
                - 1)
            .abs()
                <= 1
            {
                if let Some(bin_idx) = self.bin_idx {
                    if self.symbol_cnt == SyncState::NetId2 {
                        self.preamb_up_vals[0] = bin_idx;
                    }
                }
                self.preamb_up_vals[Into::<usize>::into(self.symbol_cnt)] = bin_idx_new;
                true
            } else {
                false
            }
        } else {
            false
        };
        if condition {
            let preamble_raw_idx_offset =
                self.m_number_of_bins * Into::<usize>::into(self.symbol_cnt);
            let count = self.m_number_of_bins;
            self.preamble_raw[preamble_raw_idx_offset..(preamble_raw_idx_offset + count)]
                .copy_from_slice(&self.in_down[0..count]);
            let preamble_raw_up_idx_offset =
                self.m_samples_per_symbol * Into::<usize>::into(self.symbol_cnt);
            let count = self.m_samples_per_symbol;
            // println!("{}", count);
            // println!("{}", preamble_raw_up_idx_offset);
            // println!("{}", (self.m_os_factor / 2));
            // println!("{}", self.preamble_raw_up.len());
            // println!("{}", input.len());
            self.preamble_raw_up[preamble_raw_up_idx_offset..(preamble_raw_up_idx_offset + count)]
                .copy_from_slice(&input[(self.m_os_factor / 2)..(self.m_os_factor / 2 + count)]);

            self.increase_symbol_count();
            // info!("symbol_cnt: {}", Into::<usize>::into(self.symbol_cnt));
        } else {
            let count = self.m_number_of_bins;
            self.preamble_raw[0..count].copy_from_slice(&self.in_down[0..count]);
            let count = self.m_samples_per_symbol;
            self.preamble_raw_up[0..count]
                .copy_from_slice(&input[(self.m_os_factor / 2)..(self.m_os_factor / 2 + count)]);

            self.transition_state(DecoderState::Detect, Some(SyncState::NetId2));
        }
        self.bin_idx = bin_idx_new_opt;
        let items_to_consume = if self.symbol_cnt == self.m_n_up_req {
            info!(
                "..:: Frame Detected ({:.1}MHz, SF{})",
                self.m_center_freq as f32 / 1.0e6,
                self.m_sf
            );
            // info!(
            //     "FrameSync: detected required nuber of upchirps ({})",
            //     Into::<usize>::into(self.m_n_up_req)
            // );
            self.additional_upchirps = 0;
            self.k_hat = most_frequent(&self.preamb_up_vals);
            let input_idx_offset = (0.75 * self.m_samples_per_symbol as f32
                - self.k_hat as f32 * self.m_os_factor as f32)
                as usize;
            let count = self.m_samples_per_symbol / 4;
            self.net_id_samp[0..count]
                .copy_from_slice(&input[input_idx_offset..(input_idx_offset + count)]);

            // perform the coarse synchronization
            self.transition_state(DecoderState::Sync, Some(SyncState::NetId1));
            self.m_os_factor as isize * (self.m_number_of_bins as isize - self.k_hat as isize)
        } else {
            // info!(
            //     "FrameSync: did not detect required nuber of upchirps ({}/{})",
            //     Into::<usize>::into(self.symbol_cnt),
            //     Into::<usize>::into(self.m_n_up_req)
            // );
            self.m_samples_per_symbol as isize
        };
        (items_to_consume, 0)
    }

    fn sync_quarter_down(
        &mut self,
        sio: &mut StreamIo,
        input: &[Complex32],
        out: &mut [Complex32],
        nitems_to_process: usize,
    ) -> (isize, usize) {
        let mut items_to_consume = 4;  // always consume the remaining four samples of the second NetId (except when the offsets added below become more negative than 1/4 symbol length), left over by the previous state as a buffer
        let mut items_to_output = 0;
        let mut sync_log_out: &mut [f32] = &mut [];
        let m_should_log = if sio.outputs().len() == 2 {
            sync_log_out = sio.output(1).slice::<f32>();
            true
        } else {
            false
        };
        let offset = items_to_consume as usize; // currently assumed start of the quarter down symbol, which might in fact already be payload (first m_samples_per_symbol already filled in sync->Down2)
        let count = self.m_samples_per_symbol;
        // requires self.m_samples_per_symbol samples in input buffer, but QuarterDown also needs 'backward' access to up to 3 samples for net-id re-synching when CFO is at minimum
        self.additional_symbol_samp[self.m_samples_per_symbol..(self.m_samples_per_symbol + count)]
            .copy_from_slice(&input[offset..(offset+count)]);
        let m_cfo_int = if let Some(down_val) = self.down_val {
            // info!("down_val: {}", down_val);
            // tuning CFO entails re-tuning STO (to not lose alignment of preamble upchirps) -> slope is normalized to 1 -> move half distance in frequency -> entails moving half distance in time -> aligns downchirp with sampling window, while keeping alignment of upchirps
            // if CFO_int is 0, this check is perfectly aligned with the second downchirp.
            // if it is less than 0, we have the full first downchirp before to still give a valid (modulated) downchirp
            // if it is higher than one, we have a quarter upchirp ahead, which is enough, as we can only (or rather assume to) be misaligned by not more than 1/4 symbol in time at this point. (re-aligning by 1/4 t_sym in time entails shifting by 1/4 BW in freqeuncy, which shifts symbols by half in the observed window due to aliasing)
            // otherwise we were off by more than 1/4 the BW in either direction, which is too much for this algorithm to compensate
            if down_val < self.m_number_of_bins / 2 {
                down_val as isize / 2
            } else {
                // if ahead by more than half a symbol, we are _probably_ actually behind by less than half a symbol
                (down_val as isize - self.m_number_of_bins as isize) / 2
            }
        } else {
            panic!("self.down_val must not be None here.")
        };

        // info!("m_cfo_int: {}", m_cfo_int);
        let cfo_int_modulo = my_modulo(m_cfo_int, self.m_number_of_bins);
        // info!(
        //     "(m_cfo_int % self.m_number_of_bins as isize): {}",
        //     cfo_int_modulo
        // );

        // *******
        // re-estimate sto_frac and estimate SFO
        // *******
        // correct STOint and CFOint in the preamble upchirps
        // correct STOint
        self.preamble_upchirps.rotate_left(cfo_int_modulo);
        // correct CFOint
        let cfo_int_correc: Vec<Complex32> = (0_usize
            ..((Into::<usize>::into(self.m_n_up_req) + self.additional_upchirps)
                * self.m_number_of_bins))
            .map(|x| {
                Complex32::from_polar(
                    1.,
                    -2. * PI * m_cfo_int as f32 / self.m_number_of_bins as f32 * x as f32,
                )
            })
            .collect();
        self.preamble_upchirps =
            volk_32fc_x2_multiply_32fc(&self.preamble_upchirps, &cfo_int_correc); // count: up_symb_to_use * m_number_of_bins
        // correct SFO in the preamble upchirps
        // SFO times symbol duration = number of samples we need to compensate the symbol duration by
        // small, as m_cfo_int+self.m_cfo_frac bounded by ]-(self.m_number_of_bins/4+0.5),+(self.m_number_of_bins/4+0.5)[
        // BW/s_f = 1/6400 @ 125kHz, 800mHz
        // -> sfo_hat ~ ]-1/250,+1/250[
        self.sfo_hat = (m_cfo_int as f32 + self.m_cfo_frac as f32) * self.m_bw as f32
            / self.m_center_freq as f32;
        // CFO normalized to carrier frequency / SFO times t_samp
        let clk_off = self.sfo_hat / self.m_number_of_bins as f32;
        let fs = self.m_bw as f32;
        // we wanted f_c_true, got f_c_true+cfo_int+cfo_fraq -> f_c = f_c_true-cfo_int-cfo_fraq -> f_c = f_c_true - (cfo_int+cfo_fraq) -> normalized "clock offset" of f_c/f_c_true=1-(cfo_int+cfo_fraq)/f_c_true
        // assume linear offset: we wanted BW, assume we got fs_p=BW-(cfo_int+cfo_fraq)/f_c_true*BW -> fs_p=BW*(1-(cfo_int+cfo_fraq)/f_c_true)
        // compute actual sampling frequency fs_p to be able to compensate
        let fs_p = fs * (1. - clk_off);
        let n = self.m_number_of_bins;
        // correct SFO
        let sfo_corr_vect: Vec<Complex32> = (0..((Into::<usize>::into(self.m_n_up_req)
            + self.additional_upchirps)
            * self.m_number_of_bins))
            .map(|x| {
                Complex32::from_polar(
                    1.,
                    -2. * PI * ((x % n) * (x % n)) as f32 / 2. / n as f32
                        * (fs / fs_p * fs / fs_p - 1.)
                        + ((x / n) as f32 * (fs / fs_p * fs / fs_p - fs / fs_p)
                            + fs / 2. * (1. / fs - 1. / fs_p))
                            * (x % n) as f32,
                )
            })
            .collect();
        let count = self.up_symb_to_use * self.m_number_of_bins;
        let tmp =
            volk_32fc_x2_multiply_32fc(&self.preamble_upchirps[0..count], &sfo_corr_vect[0..count]);
        self.preamble_upchirps[0..count].copy_from_slice(&tmp);
        // re-estimate sto_frac based on the now corrected self.preamble_upchirps to get better estimate than in the beginning of the upchirps
        let tmp_sto_frac = self.estimate_sto_frac();
        let diff_sto_frac = self.m_sto_frac - tmp_sto_frac; // both bounded by ]-0.5, 0.5] -> diff bounded by ]-1.0, 1.0[
        if diff_sto_frac.abs() <= (self.m_os_factor - 1) as f32 / self.m_os_factor as f32 {  // TODO always prevents update if os_factor = 1
            // avoid introducing off-by-one errors by estimating fine_sto=-0.499 , rough_sto=0.499
            self.m_sto_frac = tmp_sto_frac;
        }

        // get SNR estimate from preamble
        // downsample preab_raw
        // apply sto correction
        let preamble_raw_up_offset = self.m_os_factor * (self.m_number_of_bins - self.k_hat)
            - FrameSync::my_roundf(self.m_os_factor as f32 * self.m_sto_frac) as usize;
        let count = (Into::<usize>::into(self.m_n_up_req) + self.additional_upchirps)
            * self.m_number_of_bins;
        let mut corr_preamb: Vec<Complex32> = self.preamble_raw_up
            [preamble_raw_up_offset..(preamble_raw_up_offset + self.m_os_factor * count)]
            .iter()
            .step_by(self.m_os_factor)
            .copied()
            .collect();
        corr_preamb.rotate_left(cfo_int_modulo);
        // apply cfo correction
        corr_preamb = volk_32fc_x2_multiply_32fc(&corr_preamb, &cfo_int_correc);
        for i in 0..(Into::<usize>::into(self.m_n_up_req) + self.additional_upchirps) {
            let offset = self.m_number_of_bins * i;
            let end_range = self.m_number_of_bins * (i + 1);
            let tmp = volk_32fc_x2_multiply_32fc(
                &corr_preamb[offset..end_range],
                &self.cfo_frac_correc[0..self.m_number_of_bins],
            );
            corr_preamb[offset..end_range].copy_from_slice(&tmp);
        }

        // //apply sfo correction
        corr_preamb = volk_32fc_x2_multiply_32fc(&corr_preamb, &sfo_corr_vect);

        let mut snr_est = 0.0_f32;
        for i in 0..self.up_symb_to_use {
            snr_est += self.determine_snr(
                &corr_preamb[(i * self.m_number_of_bins)..((i + 1) * self.m_number_of_bins)],
            );
        }
        snr_est /= self.up_symb_to_use as f32;

        // update sto_frac to its value at the beginning of the net id
        self.m_sto_frac += self.sfo_hat * self.m_preamb_len as f32;
        // ensure that m_sto_frac is in ]-0.5,0.5]
        // STO_int is already at state of NetID 1 (due to re-synching via net_id), ignore additional int offset normally created by wrapping around the fractional offset
        // TODO evaluate, original code actually ensures it is within [-0.5, 0.5]
        // if self.m_sto_frac.abs() > 0.5 {
        //     self.m_sto_frac += if self.m_sto_frac > 0. { -1. } else { 1. };
        // }
        if self.m_sto_frac * 2.0 > 1.0 {
            self.m_sto_frac -= 1.0;
        } else if self.m_sto_frac * 2.0 <= -1.0 {
            self.m_sto_frac += 1.0;
        }
        // decim net id according to new sto_frac and sto int
        // start_off gives the offset in the net_id_samp vector required to be aligned in time (CFOint is equivalent to STOint at this point, since upchirp_val was forced to 0, and initial alignment has already been performed. note that CFOint here is only the remainder of STOint that needs to be re-aligned.)
        let start_off = (self.m_os_factor as isize / 2
            - FrameSync::my_roundf(self.m_sto_frac * self.m_os_factor as f32)
            // self.m_number_of_bins as isize / 4 is manually introduced static NET_ID_1 start offset in array before CFO alignment to be able to align negative m_cfo_int offsets
            + self.m_os_factor as isize * (self.m_number_of_bins as isize / 4 + m_cfo_int))
            as usize;
        // info!("self.m_os_factor / 2: {}", self.m_os_factor / 2);
        // info!(
        //     "FrameSync::my_roundf(self.m_sto_frac * self.m_os_factor as f32): {}",
        //     FrameSync::my_roundf(self.m_sto_frac * self.m_os_factor as f32)
        // );
        // info!(
        //     "self.m_os_factor * (self.m_number_of_bins / 4 + m_cfo_int): {}",
        //     self.m_os_factor as isize
        //         * (self.m_number_of_bins as isize / 4 + m_cfo_int)
        // );
        // info!("self.m_number_of_bins: {}", self.m_number_of_bins);
        // info!("self.m_sto_frac: {}", self.m_sto_frac);
        // info!("m_cfo_int: {}", m_cfo_int);
        // info!("start_offset: {}", start_off);
        let count = 2 * self.m_number_of_bins;
        let mut net_ids_samp_dec: Vec<Complex32> = self.net_id_samp
            [start_off..(start_off + count * self.m_os_factor)]
            .iter()
            .step_by(self.m_os_factor)
            .copied()
            .collect();
        net_ids_samp_dec = volk_32fc_x2_multiply_32fc(&net_ids_samp_dec, &cfo_int_correc);

        // correct CFO_frac in the network ids
        let tmp = volk_32fc_x2_multiply_32fc(
            &net_ids_samp_dec[0..self.m_number_of_bins],
            &self.cfo_frac_correc,
        );
        net_ids_samp_dec[0..self.m_number_of_bins].copy_from_slice(&tmp);
        let tmp = volk_32fc_x2_multiply_32fc(
            &net_ids_samp_dec[self.m_number_of_bins..(2 * self.m_number_of_bins)],
            &self.cfo_frac_correc,
        );
        net_ids_samp_dec[self.m_number_of_bins..(2 * self.m_number_of_bins)].copy_from_slice(&tmp);

        let net_id_0_tmp = FrameSync::get_symbol_val(
            &net_ids_samp_dec[0..self.m_number_of_bins],
            &self.m_downchirp,
        );
        if net_id_0_tmp.is_none() {
            warn!("Döner: encountered symbol with signal energy 0.0, aborting.");
            self.reset();
            return (items_to_consume, 0);
        }
        self.net_id[0] = net_id_0_tmp.unwrap().try_into()
        .expect("net-id can't be greater than SF bits, with SF<=12.");
        let net_id_1_tmp = FrameSync::get_symbol_val(
            &net_ids_samp_dec[self.m_number_of_bins..(2 * self.m_number_of_bins)],
            &self.m_downchirp,
        );
        if net_id_1_tmp.is_none() {
                warn!("Döner: encountered symbol with signal energy 0.0, aborting.");
                self.reset();
                return (items_to_consume, 0);
        }
        self.net_id[1]  = net_id_1_tmp.unwrap().try_into()
        .expect("net-id can't be greater than SF bits, with SF<=12.");
        let mut one_symbol_off = false;
        let mut off_by_one_id = false;

        // info!("netid1: {} (soll {})", self.net_id[0], self.m_sync_words[0]);
        // info!("netid2: {} (soll {})", self.net_id[1], self.m_sync_words[1]);

        // info!("raw netid1: {}", self.net_id[0]);
        // info!("raw netid2: {}", self.net_id[1]);
        // the last three bits of netID o and 1 are always 0 -> margin of 3 in either direction to refine CFO_int
        let net_id_off_raw = self.net_id[0] & 0x07;
        let net_id_off_raw_1 = self.net_id[1] & 0x07;
        if net_id_off_raw == 0x04 {
            info!("FrameSync: bad sync: offset > 3");
            self.reset();
            return (0, 0);
        }
        // we have detected a syntactically valid net ID,
        let net_id_off: i16 = if net_id_off_raw > 4 {
            // closer to the next higher net_id
            self.net_id[0] += 0x08;
            // frequencies outside the BW alias and wrap around, preserving the chirp structure,
            // but ONLY IF we ovwersample at the SDR to include the guard bands and then subsample
            // without a lowpass filter
            self.net_id[0] %= self.m_number_of_bins as u16;
            // assume same offset for both parts, verify below
            self.net_id[1] += 0x08;
            self.net_id[1] %= self.m_number_of_bins as u16;
            net_id_off_raw as i16 - 0x08
        } else {
            net_id_off_raw as i16
        };
        if net_id_off != 0 && net_id_off.abs() > 1 {
            warn!("[frame_sync.rs] net id offset >1: {}", net_id_off);
        }
        // discard the lower bits introduced by the offset
        self.net_id[0] &= 0xF8;
        self.net_id[1] &= 0xF8;
        if net_id_off_raw != net_id_off_raw_1 {
            // check if we are in fact checking the second net ID and that the first one was considered as a preamble upchirp
            // TODO does it make sense to require the netid to be known in this case, or is the matching offset enough to indicate we missed net_id[0] in the preamble?
            self.net_id[1] = self.net_id[0];
            let i = Into::<usize>::into(self.m_n_up_req) + self.additional_upchirps - 1;
            let net_id_1_tmp = FrameSync::get_symbol_val(
                &corr_preamb[(i * self.m_number_of_bins)..((i + 1) * self.m_number_of_bins)],
                &self.m_downchirp,
            );
            if net_id_1_tmp.is_none() {
                warn!("Döner: encountered symbol with signal energy 0.0, aborting.");
                self.reset();
                return (items_to_consume, 0);
            }
            let net_id_1_tmp: u16 = net_id_1_tmp.unwrap().try_into()
            .expect("net-id can't be greater than SF bits, with SF<=12.");
            if net_id_1_tmp & 0x07 != net_id_off_raw {
                info!(
                    "FrameSync: bad sync: different offset for recovered net_id[0] and net_id[1]"
                );
                self.reset();
                return (items_to_consume, 0);
            } else if !(self.known_valid_net_ids_reverse[self.net_id[0] as usize]
                [(net_id_1_tmp & 0xF8) as usize])
            {
                // TODO does it make sense to require the netid to be known in this case, or is the matching offset enough to indicate we missed net_id[0] in the preamble?
                info!("FrameSync: bad sync: different offset for original net_id[0] and net_id[1] and no match for recovered net_id[0]");
                self.reset();
                return (items_to_consume, 0);
            } else {
                info!("detected netid2 as netid1, recovering..");
                self.net_id[0] = net_id_1_tmp;
                // info!("netid1: {}", self.net_id[0]);
                // info!("netid2: {}", self.net_id[1]);
                one_symbol_off = true;
                if m_should_log {
                    off_by_one_id = net_id_off != 0;
                }
                items_to_consume -= -(self.m_os_factor as isize) * net_id_off as isize;
                // the first symbol was mistaken for the end of the downchirp. we should correct and output it.
                let start_off = self.m_os_factor as isize / 2  // start half a sample delayed to have a buffer for the following STOfrac (of value +-1/2 sample) ->
                    - FrameSync::my_roundf(self.m_sto_frac * self.m_os_factor as f32)
                    + self.m_os_factor as isize * (self.m_number_of_bins as isize / 4 + m_cfo_int);
                for i in (start_off..(self.m_samples_per_symbol as isize * 5 / 4))
                    .step_by(self.m_os_factor)
                {
                    // assert!((i - start_off) >= 0);
                    // assert!(i >= 0);  // first term of start_off >= 0, second term >= 0 (m_cfo_int >= -m_number_of_bins/4)
                    out[(i - start_off) as usize / self.m_os_factor] =
                        self.additional_symbol_samp[i as usize];
                }
                items_to_output = self.m_number_of_bins;
                self.frame_cnt += 1;
            }
        } else {
            info!("detected syntactically correct net_id with matching offset {net_id_off}");
            // info!("netid1: {}", self.net_id[0]);
            // info!("netid2: {}", self.net_id[1]);
            if m_should_log {
                off_by_one_id = net_id_off != 0;
            }
            // correct remaining offset in time only, as correction in frequency only necessary for estimating SFO (by first estimating CFO), and SFO is only estimated once per frame.
            // can shift by additional 3 samples -> "buffer" (not zet consumed samples up to the beginning of the payload) needs to be |min(m_cfo_int)|+3 = m_samples_per_symbol/4+3 samples long
            items_to_consume -= -(self.m_os_factor as isize) * net_id_off as isize;
            self.frame_cnt += 1;
            if !(self.known_valid_net_ids[self.net_id[0] as usize][self.net_id[1] as usize]) {
                info!(
                    "encountered new net id: [{}, {}], trying to decode header...",
                    self.net_id[0], self.net_id[1]
                );
            }
        }
        info!("SNR: {}dB", snr_est);
        // net IDs syntactically correct and matching offset => frame detected, proceed with trying to decode the header
        self.m_received_head = false;
        // consume the quarter downchirp, and at the same time correct CFOint (already applied correction for NET_ID recovery was only in retrospect on a local buffer)
        items_to_consume +=
            self.m_samples_per_symbol as isize / 4 + self.m_os_factor as isize * m_cfo_int;
        assert!(items_to_consume <= nitems_to_process as isize, "must not happen, we already altered persistent state.");
        //assert!(items_to_consume >= 0_isize, "must not happen, can not consume negative amount of samples. Implies net_id_off({net_id_off}) - m_cfo_int({m_cfo_int}) was greater than self.m_number_of_bins({})", self.m_number_of_bins);
        // info!("Frame Detected!!!!!!!!!!");
        // update sto_frac to its value at the payload beginning
        // self.m_sto_frac can now be slightly outside ]-0.5,0.5] samples
        self.m_sto_frac += self.sfo_hat * 4.25;
        // STO_int (=k_hat - CFO_int) was last updated at start of netID -> shift sto_frac back into ]-0.5,0.5] range and add possible offset of one to STO_int (by consuming one sample more or less)
        // TODO evaluate
        if self.m_sto_frac * 2.0 > 1.0 {
            self.m_sto_frac -= 1.0;
            items_to_consume += 1;
        } else if self.m_sto_frac * 2.0 <= -1.0 {
            self.m_sto_frac += 1.0;
            items_to_consume -= 1;
        }
        self.sfo_cum = ((self.m_sto_frac * self.m_os_factor as f32)
            - FrameSync::my_roundf(self.m_sto_frac * self.m_os_factor as f32) as f32)
            / self.m_os_factor as f32;

        let mut frame_info: HashMap<String, Pmt> = HashMap::new();

        frame_info.insert(String::from("is_header"), Pmt::Bool(true));
        frame_info.insert(String::from("cfo_int"), Pmt::F32(m_cfo_int as f32));
        frame_info.insert(String::from("cfo_frac"), Pmt::F64(self.m_cfo_frac));
        frame_info.insert(String::from("sf"), Pmt::Usize(self.m_sf));
        let frame_info_pmt = Pmt::MapStrPmt(frame_info);

        sio.output(0).add_tag(
            0,
            Tag::NamedAny("frame_info".to_string(), Box::new(frame_info_pmt)),
        );
        if one_symbol_off {
            self.transition_state(DecoderState::SfoCompensation, Some(SyncState::NetId2));
        } else {
            self.transition_state(DecoderState::SfoCompensation, Some(SyncState::NetId1));
        };

        if m_should_log {
            // estimate SNR

            // for i in 0..self.up_symb_to_use {
            //     snr_est2 += self.determine_snr(
            //         &self.preamble_upchirps[(i * self.m_number_of_bins)
            //             ..((i + 1) * self.m_number_of_bins)],
            //     );
            // }
            // snr_est2 /= self.up_symb_to_use as f32;
            let cfo_log = m_cfo_int as f32 + self.m_cfo_frac as f32;
            let sto_log = (self.k_hat as isize - m_cfo_int) as f32 + self.m_sto_frac;
            let srn_log = snr_est;
            let sfo_log = self.sfo_hat;

            sync_log_out[0] = srn_log;
            sync_log_out[1] = cfo_log;
            sync_log_out[2] = sto_log;
            sync_log_out[3] = sfo_log;
            sync_log_out[4] = if off_by_one_id { 1. } else { 0. };
            sio.output(1).produce(5);
        }
        // #ifdef PRINT_INFO
        //
        //                         std::cout << "[frame_sync_impl.cc] " << frame_cnt << " CFO estimate: " << m_cfo_int + m_cfo_frac << ", STO estimate: " << k_hat - m_cfo_int + m_sto_frac << " snr est: " << snr_est << std::endl;
        // #endif
        (items_to_consume, items_to_output)
    }

    fn sync(
        &mut self,
        sio: &mut StreamIo,
        input: &[Complex32],
        out: &mut [Complex32],
        nitems_to_process: usize,
    ) -> (isize, usize) {
        let mut items_to_output = 0;
        if !self.cfo_frac_sto_frac_est {
            // info!(
            //     " want {} samples, got {}",
            //     self.m_number_of_bins, self.k_hat,
            // );
            let (preamble_upchirps_tmp, cfo_frac_tmp) = self.estimate_cfo_frac_bernier(
                &self.preamble_raw[(self.m_number_of_bins - self.k_hat)..],
            );
            // let (preamble_upchirps_tmp, cfo_frac_tmp) =
            //     self.estimate_cfo_frac(&self.preamble_raw[(self.m_number_of_bins - self.k_hat)..]);
            self.preamble_upchirps = preamble_upchirps_tmp;
            self.m_cfo_frac = cfo_frac_tmp;
            self.m_sto_frac = self.estimate_sto_frac();
            // create correction vector
            self.cfo_frac_correc = (0..self.m_number_of_bins)
                .map(|x| {
                    Complex32::from_polar(
                        1.,
                        -2. * PI * self.m_cfo_frac as f32 / self.m_number_of_bins as f32 * x as f32,
                    )
                })
                .collect();
            self.cfo_frac_sto_frac_est = true;
        }
        let mut items_to_consume = self.m_samples_per_symbol as isize;
        // apply cfo correction
        let symb_corr = volk_32fc_x2_multiply_32fc(&self.in_down, &self.cfo_frac_correc);

        self.bin_idx = FrameSync::get_symbol_val(&symb_corr, &self.m_downchirp);
        match self.symbol_cnt {
            SyncState::NetId1 => {
                // assert!(
                //     nitems_to_process
                //         >= self.m_os_factor / 2
                //             + self.k_hat * self.m_os_factor
                //             + self.m_samples_per_symbol
                // );
                if nitems_to_process
                    < self.m_os_factor / 2
                        + self.k_hat * self.m_os_factor
                        + self.m_samples_per_symbol
                {
                    // warn!(
                    //     "FrameSync: not enough samples in input buffer, waiting for more."
                    // );
                    return (0, 0);
                }
                if self.bin_idx.is_some()
                    && (self.bin_idx.unwrap() == 0
                        || self.bin_idx.unwrap() == 1
                        || self.bin_idx.unwrap() == self.m_number_of_bins - 1)
                {
                    // look for additional upchirps. Won't work if network identifier 1 equals 2^sf-1, 0 or 1!
                    let input_offset = (0.75 * self.m_samples_per_symbol as f32) as usize;
                    let count = self.m_samples_per_symbol / 4;
                    self.net_id_samp[0..count]
                        .copy_from_slice(&input[input_offset..(input_offset + count)]);
                    if self.additional_upchirps >= 3 {
                        self.preamble_raw_up.rotate_left(self.m_samples_per_symbol);
                        let preamble_raw_up_offset =
                            self.m_samples_per_symbol * (Into::<usize>::into(self.m_n_up_req) + 3);
                        let input_offset = self.m_os_factor / 2 + self.k_hat * self.m_os_factor;
                        let count = self.m_samples_per_symbol;
                        self.preamble_raw_up
                            [preamble_raw_up_offset..(preamble_raw_up_offset + count)]
                            .copy_from_slice(&input[input_offset..(input_offset + count)]);
                    } else {
                        let preamble_raw_up_offset = self.m_samples_per_symbol
                            * (Into::<usize>::into(self.m_n_up_req) + self.additional_upchirps);
                        let input_offset = self.m_os_factor / 2 + self.k_hat * self.m_os_factor;
                        let count = self.m_samples_per_symbol;
                        self.preamble_raw_up
                            [preamble_raw_up_offset..(preamble_raw_up_offset + count)]
                            .copy_from_slice(&input[input_offset..(input_offset + count)]);
                        self.additional_upchirps += 1;
                    }
                } else {
                    // network identifier 1 correct or off by one
                    let net_id_samp_offset = self.m_samples_per_symbol / 4;
                    let count = self.m_samples_per_symbol;
                    self.net_id_samp[net_id_samp_offset..(net_id_samp_offset + count)]
                        .copy_from_slice(&input[0..count]);
                    // self.net_ids[0] = self.bin_idx;
                    self.transition_state(DecoderState::Sync, Some(SyncState::NetId2));
                }
            }
            SyncState::NetId2 => {
                assert!(nitems_to_process >= (self.m_number_of_bins + 1) * self.m_os_factor);
                let net_id_samp_offset = self.m_samples_per_symbol * 5 / 4;
                let count = (self.m_number_of_bins + 1) * self.m_os_factor;
                self.net_id_samp[net_id_samp_offset..(net_id_samp_offset + count)]
                    .copy_from_slice(&input[0..count]);
                // self.net_ids[1] = self.bin_idx;
                self.transition_state(DecoderState::Sync, Some(SyncState::Downchirp1));
            }
            SyncState::Downchirp1 => {
                let net_id_samp_offset = self.m_samples_per_symbol * 9 / 4;
                let count = self.m_samples_per_symbol / 4;
                self.net_id_samp[net_id_samp_offset..(net_id_samp_offset + count)]
                    .copy_from_slice(&input[0..count]);
                self.transition_state(DecoderState::Sync, Some(SyncState::Downchirp2));
            }
            SyncState::Downchirp2 => {
                self.down_val = FrameSync::get_symbol_val(&symb_corr, &self.m_upchirp);
                // info!("self.down_val: {}", self.down_val.unwrap());
                let count = self.m_samples_per_symbol;
                self.additional_symbol_samp[0..count].copy_from_slice(&input[0..count]);
                self.transition_state(DecoderState::Sync, Some(SyncState::QuarterDown));
                // do not consume the last four samples of the symbol yet, as we need an additional buffer in the following QarterDown state to cope with severely negative STO_int in combination with negative re-synching due to NetId offset and fractional STO <-0.5 due to SFO evolution
                // also work only ensures that there is one full symbol plus these four in the input queue before calling the subfunctions, so by leaving 4 samples of the second downchirp, we will have 1/4 symbol time + 4 samples to sync backward, but also a full new symbol for the additional_symbol_samp buffer
                (items_to_consume, items_to_output) = (self.m_samples_per_symbol as isize - 4, 0);
            }
            SyncState::QuarterDown => {
                (items_to_consume, items_to_output) =
                    self.sync_quarter_down(sio, input, out, nitems_to_process);
            }
            _ => warn!("encountered unexpercted symbol_cnt SyncState."),
        }
        (items_to_consume, items_to_output)
    }

    fn compensate_sfo(
        &mut self,
        sio: &mut StreamIo,
        // input: &[Complex32],
        out: &mut [Complex32],
    ) -> (isize, usize) {
        // transmit only useful symbols (at least 8 symbol for PHY header)
        if let Ok(Some(frame_info_tag)) = self.tag_from_msg_handler_to_work_channel.1.try_next() {
            // info!("new frame_info tag: {:?}", frame_info_tag);
            sio.output(0).add_tag(
                0,
                Tag::NamedAny("frame_info".to_string(), Box::new(frame_info_tag)),
            );
        }

        if Into::<usize>::into(self.symbol_cnt) < 8
            || (Into::<usize>::into(self.symbol_cnt) < self.m_symb_numb && self.m_received_head)
        {
            // info!("self.symbol_cnt: {}", Into::<usize>::into(self.symbol_cnt));
            // output downsampled signal (with no STO but with CFO)
            let count = self.m_number_of_bins;
            out[0..count].copy_from_slice(&self.in_down[0..count]);
            let mut items_to_consume = self.m_samples_per_symbol as isize;

            //   update sfo evolution
            // if cumulative SFO is greater than 1/2 sample duration
            assert!(self.sfo_cum.abs() <= 1.5 / self.m_os_factor as f32, "Döner: SFO is greater than one microsample {}", self.sfo_cum);
            if self.sfo_cum.abs() > 0.5 / self.m_os_factor as f32 {
                // sfo_cum starts out in range ]-0.5,0.5[ microsamples, sfo_hat is generally multiple orders of magnitude smaller than one microsample (at reasonable os_factors) -> sfo_cum can surpass the threshold, but not "overshoot" by more than one microsample -> it is sufficient to compensate by one microsample
                items_to_consume -= self.sfo_cum.signum() as isize;
                self.sfo_cum -= self.sfo_cum.signum() / self.m_os_factor as f32;
            }
            self.sfo_cum += self.sfo_hat;
            self.increase_symbol_count();
            (items_to_consume, self.m_number_of_bins)
        } else if !self.m_received_head {
            // TODO call again when header decoded message ('frame_info') arrives; done automatically?
            // Wait for the header to be decoded
            (0, 0)
        } else {
            // frame fully received
            self.reset();
            (self.m_samples_per_symbol as isize, 0)
        }
    }

    /// stay in the same outer state and inrease the symbol count by one
    fn increase_symbol_count(&mut self) {
        self.symbol_cnt = From::<usize>::from(Into::<usize>::into(self.symbol_cnt) + 1_usize);
    }

    /// enter a new outer state and optionally change the symbol count
    fn transition_state(
        &mut self,
        new_decoder_state: DecoderState,
        new_sync_state: Option<SyncState>,
    ) {
        self.m_state = new_decoder_state;
        if let Some(sync_state) = new_sync_state {
            self.symbol_cnt = sync_state;
        }
    }

    /// reset STO and CFO estimates and reset state to Detect the next frame
    fn reset(&mut self) {
        self.transition_state(DecoderState::Detect, Some(SyncState::NetId2));
        self.k_hat = 0;
        self.m_cfo_frac = 0.;
        self.m_sto_frac = 0.;
        self.sfo_hat = 0.;
        self.sfo_cum = 0.;
        self.cfo_frac_sto_frac_est = false;
        self.ready_to_detect = true;
    }
}

#[async_trait]
impl Kernel for FrameSync {
    async fn work(
        &mut self,
        _io: &mut WorkIo,
        sio: &mut StreamIo,
        _m: &mut MessageIo<Self>,
        _b: &mut BlockMeta,
    ) -> Result<()> {
        let out = sio.output(0).slice::<Complex32>();
        // check if there is enough space in the output buffer
        if out.len() < self.m_number_of_bins {
            return Ok(());
        }
        let input = sio.input(0).slice::<Complex32>();
        let nitems_to_process = input.len();

        // // let tags = sio.input(0).tags().iter().filter(|x| x.index < )
        // let tags: Vec<(usize, usize)> = sio
        //     .input(0)
        //     .tags()
        //     .iter()
        //     .map(|x| match x {
        //         ItemTag {
        //             index,
        //             tag: Tag::NamedAny(n, val),
        //         } => {
        //             if n == "new_frame" {
        //                 match (**val).downcast_ref().unwrap() {
        //                     Pmt::MapStrPmt(map) => {
        //                         let sf_tmp = map.get("sf").unwrap();
        //                         match sf_tmp {
        //                             Pmt::Usize(sf) => Some((*index, *sf)),
        //                             _ => None,
        //                         }
        //                     }
        //                     _ => None,
        //                 }
        //             } else {
        //                 None
        //             }
        //         }
        //         _ => None,
        //     })
        //     .filter(|x| x.is_some())
        //     .map(|x| x.unwrap())
        //     .collect();
        // //             get_tags_in_window(tags, 0, 0, ninput_items[0], pmt::string_to_symbol("new_frame"));
        // if tags.len() > 0 {
        //     if tags[0].0 != 0 {
        //         nitems_to_process = tags[0].0; // only use symbol until the next frame begin (SF might change)
        //     } else {
        //         if tags.len() >= 2 {
        //             nitems_to_process = tags[1].0 - tags[0].0;
        //         }
        //
        //         let sf = tags[0].1;
        //         self.set_sf(sf);
        //
        //         // std::cout<<"\nhamming_cr "<<tags[0].offset<<" - cr: "<<(int)m_cr<<"\n";
        //     }
        // }

        if nitems_to_process < self.m_number_of_bins + 2
            || nitems_to_process < self.m_samples_per_symbol + 4  // 4 additional samples to cope with demand of QuarterDown (4 samples "backward" and 1 symbol forward)
        {
            // TODO check, condition taken from self.forecast()
            return Ok(());
        }
        // downsampling
        // m_sto_frac is bounded by [-0.5, 0.5[ -> indexing_offset [0, self.m_os_factor]
        let indexing_offset = (self.m_os_factor / 2)
            - FrameSync::my_roundf(self.m_sto_frac * self.m_os_factor as f32);
        self.in_down = input
            [indexing_offset..(indexing_offset + self.m_number_of_bins * self.m_os_factor)]
            .iter()
            .step_by(self.m_os_factor)
            .copied()
            .collect();
        // outer state machine
        let (items_to_consume, items_to_output) = match self.m_state {
            DecoderState::Detect => {
                if !self.ready_to_detect {
                    _io.call_again = true; // TODO
                    return Ok(());
                }
                assert!(nitems_to_process >= self.m_os_factor / 2 + self.m_samples_per_symbol);
                self.detect(input)
            }
            DecoderState::Sync => self.sync(sio, input, out, nitems_to_process),
            DecoderState::SfoCompensation => self.compensate_sfo(sio, out),
        };
        assert!(items_to_consume >= 0, "tried to consume negative amount of samples ({items_to_consume})");
        assert!(nitems_to_process >= items_to_consume as usize, "tried to consume {items_to_consume} samples, but input buffer only holds {nitems_to_process}.");
        if items_to_consume > 0 {
            println!("consumed {} samples", items_to_consume);
            sio.input(0).consume(items_to_consume as usize);
        }
        if items_to_output > 0 {
            sio.output(0).produce(items_to_output);
        } else if items_to_consume == 0 && items_to_output == 0 {
            if self.ready_to_detect && self.m_state == DecoderState::Detect {
                // no samples consumed due to sync failure (wrong net id), but not waiting for any external event (message) to trigger new call -> set call_again manually
                _io.call_again = true;
            }
        }
        if out.len() != 0 && input.len() != 0 {
            _io.call_again = true;
        }
        Ok(())
    }
}

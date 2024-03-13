// #include <iomanip>
// #include <numeric>
// #include <gnuradio/expj.h>
// #include <volk/volk.h>
// #include <algorithm>
use futuresdr::num_complex::Complex32;
use futuresdr::runtime::Pmt;
use futuresdr::runtime::{ItemTag, Tag};
use ordered_float::OrderedFloat;
use rustfft::num_traits::Float;

use std::cmp::Eq;
use std::collections::HashMap;
use std::f32::consts::PI;
use std::hash::Hash;
use std::ops::Mul;

// pub const RESET: &str = "\033[0m";
// pub const RED: &str = "\033[31m"; /* Red */
pub const MIN_SF: usize = 5; //minimum and maximum SF
pub const MAX_SF: usize = 12;

pub type LLR = f64; //< Log-Likelihood Ratio type
                    //typedef long double LLR; // 16 Bytes

// #[repr(usize)]
// #[derive(Debug, Copy, Clone)]
// pub enum Symbol_type {
//     VOID,
//     UPCHIRP,
//     SYNC_WORD,
//     DOWNCHIRP,
//     QUARTER_DOWN,
//     PAYLOAD,
//     UNDETERMINED,
// }

pub const LDRO_MAX_DURATION_MS: f32 = 16.;
pub const CW_NBR: usize = 16; // In LoRa, always "only" 16 possible codewords => compare with all and take argmax

pub const WHITENING_SEQ: [u8; 255] = [
    0xFF, 0xFE, 0xFC, 0xF8, 0xF0, 0xE1, 0xC2, 0x85, 0x0B, 0x17, 0x2F, 0x5E, 0xBC, 0x78, 0xF1, 0xE3,
    0xC6, 0x8D, 0x1A, 0x34, 0x68, 0xD0, 0xA0, 0x40, 0x80, 0x01, 0x02, 0x04, 0x08, 0x11, 0x23, 0x47,
    0x8E, 0x1C, 0x38, 0x71, 0xE2, 0xC4, 0x89, 0x12, 0x25, 0x4B, 0x97, 0x2E, 0x5C, 0xB8, 0x70, 0xE0,
    0xC0, 0x81, 0x03, 0x06, 0x0C, 0x19, 0x32, 0x64, 0xC9, 0x92, 0x24, 0x49, 0x93, 0x26, 0x4D, 0x9B,
    0x37, 0x6E, 0xDC, 0xB9, 0x72, 0xE4, 0xC8, 0x90, 0x20, 0x41, 0x82, 0x05, 0x0A, 0x15, 0x2B, 0x56,
    0xAD, 0x5B, 0xB6, 0x6D, 0xDA, 0xB5, 0x6B, 0xD6, 0xAC, 0x59, 0xB2, 0x65, 0xCB, 0x96, 0x2C, 0x58,
    0xB0, 0x61, 0xC3, 0x87, 0x0F, 0x1F, 0x3E, 0x7D, 0xFB, 0xF6, 0xED, 0xDB, 0xB7, 0x6F, 0xDE, 0xBD,
    0x7A, 0xF5, 0xEB, 0xD7, 0xAE, 0x5D, 0xBA, 0x74, 0xE8, 0xD1, 0xA2, 0x44, 0x88, 0x10, 0x21, 0x43,
    0x86, 0x0D, 0x1B, 0x36, 0x6C, 0xD8, 0xB1, 0x63, 0xC7, 0x8F, 0x1E, 0x3C, 0x79, 0xF3, 0xE7, 0xCE,
    0x9C, 0x39, 0x73, 0xE6, 0xCC, 0x98, 0x31, 0x62, 0xC5, 0x8B, 0x16, 0x2D, 0x5A, 0xB4, 0x69, 0xD2,
    0xA4, 0x48, 0x91, 0x22, 0x45, 0x8A, 0x14, 0x29, 0x52, 0xA5, 0x4A, 0x95, 0x2A, 0x54, 0xA9, 0x53,
    0xA7, 0x4E, 0x9D, 0x3B, 0x77, 0xEE, 0xDD, 0xBB, 0x76, 0xEC, 0xD9, 0xB3, 0x67, 0xCF, 0x9E, 0x3D,
    0x7B, 0xF7, 0xEF, 0xDF, 0xBF, 0x7E, 0xFD, 0xFA, 0xF4, 0xE9, 0xD3, 0xA6, 0x4C, 0x99, 0x33, 0x66,
    0xCD, 0x9A, 0x35, 0x6A, 0xD4, 0xA8, 0x51, 0xA3, 0x46, 0x8C, 0x18, 0x30, 0x60, 0xC1, 0x83, 0x07,
    0x0E, 0x1D, 0x3A, 0x75, 0xEA, 0xD5, 0xAA, 0x55, 0xAB, 0x57, 0xAF, 0x5F, 0xBE, 0x7C, 0xF9, 0xF2,
    0xE5, 0xCA, 0x94, 0x28, 0x50, 0xA1, 0x42, 0x84, 0x09, 0x13, 0x27, 0x4F, 0x9F, 0x3F, 0x7F,
];

#[repr(usize)]
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum LdroMode {
    DISABLE = 0,
    ENABLE = 1,
    AUTO = 2,
}
impl From<usize> for LdroMode {
    fn from(orig: usize) -> Self {
        match orig {
            0_usize => LdroMode::DISABLE,
            1_usize => LdroMode::ENABLE,
            2_usize => LdroMode::AUTO,
            _ => panic!("invalid value to ldro_mode"),
        }
    }
}
// /**
//  *  \brief  return the modulus a%b between 0 and (b-1)
//  */
// #[inline]
// inline long mod(long a, long b)
// { return (a%b+b)%b; }  // TODO ???

// inline double double_mod(double a, long b)
// { return fmod(fmod(a,b)+b,b);}

/**
 *  \brief  Convert an integer into a MSB first vector of bool
 *
 *  \param  integer
 *          The integer to convert
 *  \param  n_bits
 *          The output number of bits
 */
#[inline]
pub fn int2bool(integer: u16, n_bits: usize) -> Vec<bool> {
    let mut vec: Vec<bool> = vec![false; n_bits];
    let mut j = n_bits;
    for i in 0_usize..n_bits {
        j -= 1;
        vec[j] = ((integer >> i) & 1) != 0;
    }
    vec
}
/**
 *  \brief  Convert a MSB first vector of bool to a integer
 *
 *  \param  b
 *          The boolean vector to convert
 */
#[inline]
pub fn bool2int(b: &[bool]) -> u16 {
    assert!(b.len() <= 16);
    b.iter()
        .map(|x| *x as u16)
        .zip((0_usize..b.len()).rev())
        .map(|(bit, order)| bit << order)
        .sum::<u16>()
}

/**
 *  \brief  Return an modulated upchirp using s_f=bw
 *
 *  \param  chirp
 *          The pointer to the modulated upchirp
 *  \param  id
 *          The number used to modulate the chirp
 * \param   sf
 *          The spreading factor to use
 * \param os_factor
 *          The oversampling factor used to generate the upchirp
 */
#[inline]
pub fn build_upchirp(id: usize, sf: usize, os_factor: usize) -> Vec<Complex32> {
    let n = (1 << sf) as f32;
    let n_idx = 1 << sf;
    let n_fold = n * os_factor as f32 - (id * os_factor) as f32;
    if n < n_fold {
        (0..(n_idx * os_factor))
            .map(|i| i as f32)
            .map(|j| {
                Complex32::new(1.0, 0.0)
                    * Complex32::from_polar(
                        1.,
                        2.0 * PI * (j * j / (2. * n) + (id as f32 / n - 0.5) * j),
                    )
            })
            .collect()
    } else {
        (0..(n_idx * os_factor))
            .map(|i| i as f32)
            .map(|j| {
                Complex32::new(1.0, 0.0)
                    * Complex32::from_polar(
                        1.,
                        2.0 * PI * (j * j / (2. * n) + (id as f32 / n - 1.5) * j),
                    )
            })
            .collect()
    }
}

#[inline]
pub fn my_modulo(val1: isize, val2: usize) -> usize {
    if val1 >= 0 {
        (val1 as usize) % val2
    } else {
        (val2 as isize + (val1 % val2 as isize)) as usize % val2
    }
}

/**
 *  \brief  Return the reference chirps using s_f=bw
 *
 *  \param  upchirp
 *          The pointer to the reference upchirp
 *  \param  downchirp
 *          The pointer to the reference downchirp
 * \param   sf
 *          The spreading factor to use
 */
#[inline]
pub fn build_ref_chirps(sf: usize, os_factor: usize) -> (Vec<Complex32>, Vec<Complex32>) {
    let upchirp = build_upchirp(0, sf, os_factor);
    let downchirp = volk_32fc_conjugate_32fc(&upchirp);
    (upchirp, downchirp)
}

// find most frequency number in vector
#[inline]
pub fn most_frequent<T>(input_slice: &[T]) -> T
where
    T: Eq + Hash + Copy,
{
    input_slice
        .iter()
        .fold(HashMap::<T, usize>::new(), |mut map, val| {
            map.entry(*val)
                .and_modify(|frq| *frq += 1_usize)
                .or_insert(1_usize);
            map
        })
        .iter()
        .max_by(|(_, val_a), (_, val_b)| val_a.cmp(val_b))
        .map(|(k, _)| k)
        .unwrap_or_else(|| panic!("lora::utilities::most_frequent was called on empty slice."))
        .to_owned()
}

// inline std::string random_string(int Nbytes){
// const char* charmap = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
// const size_t charmapLength = strlen(charmap);
// auto generator = [&](){ return charmap[rand()%charmapLength]; };
// std::string result;
// result.reserve(Nbytes);
// std::generate_n(std::back_inserter(result), Nbytes, generator);
// return result;
//     }

pub fn argmax_float<T: Float>(input_slice: &[T]) -> usize {
    input_slice
        .iter()
        .map(|x| OrderedFloat::<T>(*x))
        .enumerate()
        .max_by(|(_, value0), (_, value1)| value0.cmp(value1))
        .map(|(idx, _)| idx)
        .unwrap_or(0_usize)
}

// TODO possibly limit size of input slices according to 'num_points' param from cpp code
pub fn volk_32fc_conjugate_32fc(a_vector: &Vec<Complex32>) -> Vec<Complex32> {
    // TODO use map and generic type
    let mut b_vector = vec![Complex32::from(0.); a_vector.len()];
    for i in 0_usize..a_vector.len() {
        let tmp: Complex32 = a_vector[i];
        b_vector[i] = tmp.conj();
    }
    b_vector
}

#[inline]
pub fn volk_32fc_x2_multiply_32fc<T: Copy + Mul<T, Output = T>>(
    input_slice_1: &[T],
    input_slice_2: &[T],
) -> Vec<T> {
    let tmp: Vec<T> = input_slice_1
        .iter()
        .zip(input_slice_2.iter())
        .map(|(x, y)| *x * *y)
        .collect();
    tmp
}

// #[inline]
// pub fn volk_32fc_x2_multiply_32fc<T: Copy + Mul<T, Output = T>, I>(
//     input_slice_1: &[T],
//     input_slice_2: &[T],
// ) -> I
// where
//     I: Iterator<Item = T>,
// {
//     input_slice_1
//         .iter()
//         .zip(input_slice_2.iter())
//         .map(|(x, y)| *x * *y)
// }

#[inline]
pub fn volk_32fc_magnitude_squared_32f(input_slice: &[Complex32]) -> Vec<f32> {
    let tmp: Vec<f32> = input_slice
        .iter()
        .map(|x| x.re * x.re + x.im * x.im)
        .collect();
    tmp
}

pub trait TryDowncast<T: Clone> {
    fn try_downcast(&self) -> Option<T> {
        None
    }
}

impl TryDowncast<String> for String {
    fn try_downcast(&self) -> Option<String> {
        Some(self.clone())
    }
}

impl TryDowncast<usize> for String {}

impl TryDowncast<f32> for String {}

impl TryDowncast<Vec<u8>> for String {}

impl TryDowncast<String> for usize {}

impl TryDowncast<usize> for usize {
    fn try_downcast(&self) -> Option<usize> {
        Some(*self)
    }
}

impl TryDowncast<f32> for usize {}

impl TryDowncast<Vec<u8>> for usize {}

impl TryDowncast<String> for f32 {}

impl TryDowncast<usize> for f32 {}

impl TryDowncast<f32> for f32 {
    fn try_downcast(&self) -> Option<f32> {
        Some(*self)
    }
}

impl TryDowncast<Vec<u8>> for f32 {}

impl TryDowncast<String> for Vec<u8> {}

impl TryDowncast<usize> for Vec<u8> {}

impl TryDowncast<f32> for Vec<u8> {}

impl TryDowncast<Vec<u8>> for Vec<u8> {
    fn try_downcast(&self) -> Option<Vec<u8>> {
        Some(self.clone())
    }
}

pub fn get_tags_in_window<T: Clone>(
    tags_in: &[ItemTag],
    window_size: usize,
    tag_name: &str,
) -> Vec<(usize, T)>
where
    String: TryDowncast<T>,
    usize: TryDowncast<T>,
    f32: TryDowncast<T>,
    Vec<u8>: TryDowncast<T>,
{
    let tags: Vec<(usize, T)> = tags_in
        .iter()
        .filter_map(|x| match x {
            ItemTag {
                index,
                tag: Tag::NamedAny(n, val),
            } => {
                if n == tag_name && *index < window_size {
                    match (**val).downcast_ref().unwrap() {
                        Pmt::String(payload) => Some((*index, payload.try_downcast().unwrap())),
                        Pmt::Usize(payload) => Some((*index, payload.try_downcast().unwrap())),
                        Pmt::F32(payload) => Some((*index, payload.try_downcast().unwrap())),
                        Pmt::Blob(payload) => Some((*index, payload.try_downcast().unwrap())),
                        _ => None,
                    }
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect();
    tags
}

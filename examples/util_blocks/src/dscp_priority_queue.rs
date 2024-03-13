// use futuresdr::futures::SinkExt;
use std::cmp::Ord;
use std::collections::{HashMap, VecDeque};
use std::fmt::{Binary, Display};
use std::hash::Hash;

use futuresdr::log::{debug, warn};

pub struct BoundedDiscretePriorityQueue<'a, T1, T2> {
    data: VecDeque<T1>,
    priority_index_map: HashMap<T2, usize>,
    priority_values: &'a [T2],
    max_size: usize,
}

impl<T1, T2> BoundedDiscretePriorityQueue<'_, T1, T2>
where
    T1: std::fmt::Debug,
    T2: Eq + Hash + Copy + Display + Binary + Ord,
{
    pub fn new(
        new_max_size: usize,
        new_priority_values: &[T2],
    ) -> BoundedDiscretePriorityQueue<T1, T2> {
        BoundedDiscretePriorityQueue {
            data: VecDeque::with_capacity(new_max_size),
            priority_index_map: HashMap::from_iter(
                new_priority_values.iter().map(|x| (*x, 0_usize)),
            ),
            priority_values: new_priority_values,
            max_size: new_max_size,
        }
    }

    pub fn flush(&mut self) {
        self.data.clear();
        for priority_key in self.priority_values {
            self.priority_index_map.insert(*priority_key, 0_usize);
        }
    }

    pub fn pop_front(&mut self) -> Option<T1> {
        if self.data.is_empty() {
            None
        } else {
            for priority_key in self.priority_values {
                self.priority_index_map.insert(
                    *priority_key,
                    self.priority_index_map[priority_key]
                        .checked_sub(1_usize)
                        .unwrap_or(0_usize),
                );
            }
            self.data.pop_front()
        }
    }

    pub fn pop_front_with_condition(&mut self, condition: Box<dyn Fn(&T1) -> bool>) -> Option<T1> {
        if self.data.is_empty() {
            None
        } else if self.data.get(0).map_or(false, |x| condition(x)) {
            for priority_key in self.priority_values {
                self.priority_index_map.insert(
                    *priority_key,
                    self.priority_index_map[priority_key]
                        .checked_sub(1_usize)
                        .unwrap_or(0_usize),
                );
            }
            self.data.pop_front()
        } else {
            None
        }
    }

    pub fn push_back(&mut self, value: T1, priority: T2) {
        match self.priority_index_map.get(&priority) {
            Some(insert_index_ref) => {
                let insert_index = *insert_index_ref;
                if insert_index >= self.max_size {
                    debug!(
                        "Input Queue: max number of frames of higher priority already in TX queue, DROPPING new frame (trying to insert at {}, queue capacity {}, priority {:#8b}).",
                        insert_index, self.max_size, priority
                    );
                    return;
                }
                // println!("inserted at index {}, priority {}", insert_index, priority);
                self.data.insert(insert_index, value);
                debug!(
                    "inserted new frame at position {}, queue len {}",
                    insert_index,
                    self.data.len()
                );
                let mut queue_full = false;
                // let mut highest_index = 0;
                for priority_key in self.priority_values {
                    if *priority_key <= priority {
                        let new_index = self.priority_index_map[priority_key] + 1_usize;
                        if new_index > self.max_size {
                            queue_full = true;
                        } else {
                            self.priority_index_map.insert(*priority_key, new_index);
                        }
                        // highest_index = highest_index.max(new_index)
                    }
                }
                // if highest_index > 20 {
                //     println!("WARNING: more than 20 samples in queue: {}", highest_index);  // TODO
                // }
                if queue_full {
                    debug!(
                        "Input Queue: max number of frames of higher or equal priority already in TX queue, DROPPING oldest frame of lowest priority. (trying to insert at {}, queue capacity {}, priority {:#8b}).",
                        insert_index, self.max_size, priority
                    );
                    self.data.pop_back();
                }
            }
            None => warn!(
                "Packet contained an invalid DSCP value: {:#8b}. DROPPING. full packet: {:?}",
                priority, value
            ),
        }
    }
}

/// Maximum number of frames to queue for transmission
pub const PRIORITY_VALUES: [u8; 21] = [
    0b000000 << 2,
    0b001000 << 2,
    0b001010 << 2,
    0b001100 << 2,
    0b001110 << 2,
    0b010000 << 2,
    0b010010 << 2,
    0b010100 << 2,
    0b010110 << 2,
    0b011000 << 2,
    0b011010 << 2,
    0b011100 << 2,
    0b011110 << 2,
    0b100000 << 2,
    0b100010 << 2,
    0b100100 << 2,
    0b100110 << 2,
    0b101000 << 2,
    0b101110 << 2, // EF
    0b110000 << 2, // CS6
    0b111000 << 2, // CS7
];

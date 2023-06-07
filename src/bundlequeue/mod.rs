pub mod allower;
pub mod scorer;

use bp7::ByteBuffer;
use prettytable::{row, Table};
use priority_queue::PriorityQueue;
use std::borrow::{Borrow, BorrowMut};
use std::collections::hash_map::DefaultHasher;
use std::fmt::{Display, Formatter};
use std::hash::{Hash, Hasher};

/// Bundle represents a wrapper around the bp7 bundle and adds hash and eq function.
#[derive(Clone)]
struct Bundle(bp7::bundle::Bundle);

impl Hash for Bundle {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Hash::hash_slice(self.0.id().as_bytes(), state);
    }
}

impl PartialEq<Self> for Bundle {
    fn eq(&self, other: &Self) -> bool {
        self.0.id() == other.0.id()
    }
}

impl Eq for Bundle {}

#[derive(Clone, Hash, Eq, PartialEq)]
pub struct NoMeta {}

impl Display for NoMeta {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("NoMeta")
    }
}

/// QueueEntry represents a generic entry in the priority queue that optionally holds a bundle
/// and additional meta data.
#[derive(Clone)]
struct QueueEntry<T: Hash + Eq + Clone + Send + Display> {
    base_priority: i32,
    bndl: Option<Bundle>,
    meta: Option<T>,
}

impl<T: Hash + Eq + Clone + Send + Display> Hash for QueueEntry<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        if self.bndl.is_some() {
            self.bndl.clone().unwrap().hash(state);
        }
        if self.meta.is_some() {
            self.meta.clone().unwrap().hash(state);
        }
    }
}

impl<T: Hash + Eq + Clone + Send + Display> PartialEq<Self> for QueueEntry<T> {
    fn eq(&self, other: &Self) -> bool {
        self.meta.is_some()
            && other.meta.is_some()
            && self.meta.as_ref().unwrap().eq(other.meta.as_ref().unwrap())
            && self.bndl.is_some()
            && other.bndl.is_some()
            && self.bndl.as_ref().unwrap().eq(other.bndl.as_ref().unwrap())
    }
}

impl<T: Hash + Eq + Clone + Send + Display> Eq for QueueEntry<T> {}

/// A Scorer is responsible for calculating a positive or negative priority score for a bundle.
pub trait Scorer<T: Hash + Eq + Clone + Send> {
    fn score(&self, bndl: &mut Option<bp7::bundle::Bundle>, meta: &mut Option<T>) -> i32;
}

/// A Allower function is responsible for checking if a bundle can be queued.
pub trait Allower<T: Hash + Eq + Clone + Send> {
    fn allow(&self, bndl: &mut Option<bp7::bundle::Bundle>, meta: &mut Option<T>) -> bool;
}

pub struct BundleQueue<T: Hash + Eq + Clone + Send + Display> {
    queue: PriorityQueue<QueueEntry<T>, i32>,
    scorer: Vec<Box<dyn Scorer<T> + Send>>,
    allower: Vec<Box<dyn Allower<T> + Send>>,
}

impl<T: Hash + Eq + Clone + Send + Display> BundleQueue<T> {
    pub(crate) fn new() -> BundleQueue<T> {
        BundleQueue {
            queue: PriorityQueue::new(),
            scorer: Vec::new(),
            allower: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn priority(&self, base_priority: i32, bndl: &Option<bp7::bundle::Bundle>, meta: &Option<T>) -> (bool, i32) {
        if bndl.is_none() && meta.is_none() {
            return (false, 0);
        };

        // check if any allower denies the bundle
        if !self
            .allower
            .iter()
            .all(|allower| -> bool { allower.allow(bndl.clone().borrow_mut(), meta.clone().borrow_mut()) })
        {
            return (false, 0);
        }

        // generate the priority score of the bundle
        let mut prio: i32 = self
            .scorer
            .iter()
            .map(|scorer| -> i32 {
                // TODO: can we avoid the clone?
                scorer.score(bndl.clone().borrow_mut(), meta.clone().borrow_mut())
            })
            .sum();
        prio += base_priority;

        (true, prio)
    }

    pub fn push(&mut self, base_priority: i32, bndl: Option<bp7::bundle::Bundle>, meta: Option<T>) -> (bool, i32) {
        let res = self.priority(base_priority, bndl.borrow(), meta.borrow());

        if !res.0 {
            return res;
        }

        self.queue.push(
            QueueEntry {
                base_priority,
                bndl: bndl.map(Bundle),
                meta,
            },
            res.1,
        );

        res
    }

    pub fn push_buf(&mut self, base_priority: i32, bndl: ByteBuffer, meta: Option<T>) -> (bool, i32) {
        match bp7::bundle::Bundle::try_from(bndl) {
            Ok(bndl) => self.push(base_priority, Some(bndl), meta),
            Err(_) => self.push(base_priority, None, meta),
        }
    }

    pub fn pop(&mut self) -> Option<(Option<bp7::bundle::Bundle>, Option<T>, i32)> {
        match self.queue.pop() {
            Some(res) => Some((
                match res.0.bndl {
                    Some(bndl) => Some(bndl.0),
                    None => None,
                },
                res.0.meta,
                res.1,
            )),
            None => None,
        }
    }

    pub fn rescore(&mut self) {
        let queue = self.queue.clone();
        let all_elements: Vec<(&QueueEntry<T>, &i32)> = queue.iter().collect();

        self.queue.clear();
        for e in all_elements.iter() {
            let bndl = match e.0.clone().bndl {
                None => None,
                Some(bndl) => Some(bndl.0),
            };

            if !self
                .allower
                .iter()
                .all(|allower| -> bool { allower.allow(bndl.clone().borrow_mut(), e.0.meta.clone().borrow_mut()) })
            {
               continue;
            }

            let res = self.priority(e.0.base_priority, bndl.borrow(), e.0.meta.borrow());

            if res.0 {
                self.queue.push(e.0.clone(), res.1);
            }
        }
    }

    pub fn add_scorer(&mut self, scorer: Box<dyn Scorer<T> + Send>) {
        self.scorer.push(scorer);
    }

    pub fn add_allower(&mut self, allower: Box<dyn Allower<T> + Send>) {
        self.allower.push(allower);
    }

    pub fn print_state(&self) -> String {
        let mut table = Table::new();

        table.add_row(row!["Bundle", "Meta", "Hash", "Priority"]);

        for e in self.queue.clone().into_sorted_iter() {
            let bundle_info = match e.0.clone().bndl {
                None => "".to_string(),
                Some(bndl) => format!("{} (/{})", bndl.0.id(), bndl.0.primary.destination.service_name().unwrap_or_else(|| "".to_string()),),
            };

            let meta = match e.0.clone().meta {
                None => "".to_string(),
                Some(meta) => format!("{}", meta).to_string(),
            };

            let mut hash = DefaultHasher::new();
            e.0.hash(&mut hash);

            table.add_row(row![bundle_info, meta, format!("{:x}", hash.finish()), e.1]);
        }

        table.to_string()
    }
}

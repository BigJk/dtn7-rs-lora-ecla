use crate::bundlequeue::{Scorer};
use bp7::Bundle;
use std::borrow::BorrowMut;
use std::collections::HashMap;
use std::hash::Hash;

/// ScoreBundleSize scores a dtn bundle based on it's size.
pub struct ScoreBundleSize {
    /// min size
    min: i32,

    /// the basic priority if size <= min
    base_prio: i32,

    /// the points to get or lose per positive deviation from min
    per_deviation: i32,
}

impl ScoreBundleSize {
    pub fn new(min: i32, base_prio: i32, per_deviation: i32) -> ScoreBundleSize {
        ScoreBundleSize {
            min,
            base_prio,
            per_deviation,
        }
    }
}

impl<T: Hash + Eq + Clone + Send> Scorer<T> for ScoreBundleSize {
    fn score(&self, bndl: &mut Option<bp7::bundle::Bundle>, _: &mut Option<T>) -> i32 {
        if bndl.is_none() {
            return 0;
        }

        let size = bndl.clone().unwrap().to_cbor().len();
        self.base_prio + ((size as f32 - self.min as f32).max(0.0) * self.per_deviation as f32) as i32
    }
}

/// ScoreEndpointService gives specific services (dtn://nodeX/service) score points.
pub struct ScoreEndpointService {
    services: HashMap<String, i32>,
}

impl ScoreEndpointService {
    pub fn new(services: HashMap<String, i32>) -> ScoreEndpointService {
        ScoreEndpointService { services }
    }
}

impl<T: Hash + Eq + Clone + Send> Scorer<T> for ScoreEndpointService {
    fn score(&self, bndl: &mut Option<Bundle>, _: &mut Option<T>) -> i32 {
        if bndl.is_none() {
            return 0;
        }

        let service = bndl
            .clone()
            .unwrap()
            .primary
            .destination
            .service_name()
            .unwrap_or("".to_string());

        *self.services.get(service.as_str()).unwrap_or(&0)
    }
}

/// ScoreMetaCheck allows to evaluate the meta values score based on some function.
pub struct ScoreMetaCheck<T: Hash + Eq + Clone + Send> {
    check: fn(&mut T) -> i32,
}

impl<T: Hash + Eq + Clone + Send> ScoreMetaCheck<T> {
    pub fn new(check: fn(&mut T) -> i32) -> ScoreMetaCheck<T> {
        ScoreMetaCheck { check }
    }
}

impl<T: Hash + Eq + Clone + Send> Scorer<T> for ScoreMetaCheck<T> {
    fn score(&self, _: &mut Option<Bundle>, meta: &mut Option<T>) -> i32 {
        if meta.is_none() {
            return 0;
        }

        (self.check)(meta.clone().unwrap().borrow_mut())
    }
}

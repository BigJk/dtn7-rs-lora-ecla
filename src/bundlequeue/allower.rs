use crate::bundlequeue::Allower;
use bp7::Bundle;
use std::borrow::BorrowMut;
use std::hash::Hash;

/// AllowBundleSize denies any bundles that exceed the max length.
pub struct AllowBundleSize {
    max: i32,
}

impl AllowBundleSize {
    pub fn new(max: i32) -> AllowBundleSize {
        AllowBundleSize { max }
    }
}


impl<T: Hash + Eq + Clone + Send> Allower<T> for AllowBundleSize {
    fn allow(&self, bndl: &mut Option<bp7::bundle::Bundle>, _: &mut Option<T>) -> bool {
        if bndl.is_none() {
            return true;
        }

        bndl.clone().unwrap().to_cbor().len() < self.max as usize
    }
}

/// AllowStillAlive denies any bundles that have exceeded their lifetime.
pub struct AllowStillAlive {}

impl AllowStillAlive {
    pub fn new() -> AllowStillAlive {
        AllowStillAlive {}
    }
}

impl<T: Hash + Eq + Clone + Send> Allower<T> for AllowStillAlive {
    fn allow(&self, bundle: &mut Option<Bundle>, _: &mut Option<T>) -> bool {
        if bundle.is_none() {
            return true;
        }

        return !bundle.as_ref().unwrap().primary.is_lifetime_exceeded();
    }
}

/// AllowMetaCheck allows to evaluate the meta value based on some function.
pub struct AllowMetaCheck<T: Hash + Eq + Clone + Send> {
    check: fn(&mut T) -> bool,
}

impl<T: Hash + Eq + Clone + Send> AllowMetaCheck<T> {
    pub fn new(check: fn(&mut T) -> bool) -> AllowMetaCheck<T> {
        AllowMetaCheck { check }
    }
}

impl<T: Hash + Eq + Clone + Send> Allower<T> for AllowMetaCheck<T> {
    fn allow(&self, _: &mut Option<Bundle>, meta: &mut Option<T>) -> bool {
        if meta.is_none() {
            return true;
        }

        (self.check)(meta.clone().unwrap().borrow_mut())
    }
}

/// AllowMetaBundleCheck allows to evaluate the meta and bundle value based on some function.
pub struct AllowMetaBundleCheck<T: Hash + Eq + Clone + Send> {
    check: fn(&mut T, &mut Option<Bundle>) -> bool,
}

impl<T: Hash + Eq + Clone + Send> AllowMetaBundleCheck<T> {
    pub fn new(check: fn(&mut T, &mut Option<Bundle>) -> bool) -> AllowMetaBundleCheck<T> {
        AllowMetaBundleCheck { check }
    }
}

impl<T: Hash + Eq + Clone + Send> Allower<T> for AllowMetaBundleCheck<T> {
    fn allow(&self, bndl: &mut Option<Bundle>, meta: &mut Option<T>) -> bool {
        if meta.is_none() {
            return true;
        }

        (self.check)(meta.clone().unwrap().borrow_mut(), bndl.clone().borrow_mut())
    }
}

//! Compile-time checks of the matcher's privacy and threading guarantees (architecture §8).
//!
//! `KeyBuffer` must not be formattable, serialisable or duplicable. Each assertion below fails
//! to compile (ambiguous impl) if `KeyBuffer` gains the named trait. The `compile_fail`
//! doctests on `KeyBuffer` check the same from the other side.

use iamb_core::matcher::{KeyBuffer, Matcher};

/// Fails to compile if `$ty` implements `$tr`: with the impl, two blanket impls apply and the
/// type parameter of `AmbiguousIfImpl` cannot be inferred.
macro_rules! assert_not_impl {
    ($ty:ty: $($tr:path),+ $(,)?) => {
        $(
            const _: fn() = || {
                trait AmbiguousIfImpl<A> {
                    fn some_item() {}
                }
                impl<T: ?Sized> AmbiguousIfImpl<()> for T {}
                #[allow(dead_code)]
                struct Invalid;
                impl<T: ?Sized + $tr> AmbiguousIfImpl<Invalid> for T {}
                let _ = <$ty as AmbiguousIfImpl<_>>::some_item;
            };
        )+
    };
}

assert_not_impl!(
    KeyBuffer: std::fmt::Debug,
    std::fmt::Display,
    Clone,
    Copy,
    serde::Serialize,
);

const fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn matcher_is_send_and_sync() {
    assert_send_sync::<Matcher>();
}

#[test]
fn key_buffer_exposes_only_its_length() {
    let mut b = KeyBuffer::default();
    b.push('a');
    b.push('b');
    assert_eq!(b.len(), 2);
    assert!(!b.is_empty());
    b.clear();
    assert!(b.is_empty());
}

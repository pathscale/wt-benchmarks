//! Regression guard for the silent `SequentialMap::prefix` defect.
//!
//! **The defect.** `prefix` takes `K::Read<'k>`. For a `BoxedStr<NonNull>` key
//! there were two `From` impls that produce one, and **both compiled**
//! (`arctic-wt/src/raw/key/unsized/boxed_slice.rs`):
//!
//! * `From<&'k str>` went through `Reader::new_prefix`, leaving `terminate`
//!   false, so the reader meant exactly the bytes given. Correct.
//! * `From<&'k Slice<I, R>>` set `terminate: I::Terminate::TRUE`, so the
//!   reader meant the bytes given **plus the invariant's terminator**.
//!
//! Under `NonNull` that terminator is a null byte and no stored key may
//! contain one, so a terminated reader could only ever match a stored key
//! exactly. `prefix` with a validated key silently degenerated into `get`: a
//! proper prefix selected nothing, a whole key selected itself. It did not
//! error and it did not panic, so a caller reading the result saw "nothing
//! under this parent" - a false factual claim rather than a failure. Neither
//! doc comment warned; `concurrent/map.rs` said the opposite of a warning,
//! "prefix does not need to satisfy any particular invariants".
//!
//! Measured on arctic-wt 0.1.7, four keys under two parents:
//!
//! | call form                             | "fn:dot" | "fn:dot/loop:0" |
//! | ------------------------------------- | -------- | --------------- |
//! | `prefix("fn:dot".into())`             |        3 |               1 |
//! | `prefix(Str::new("fn:dot")?.into())`  |        0 |               1 |
//!
//! **The fix.** arctic-wt aa15dc3, "Fix validated string prefix scans", adds
//! `Read::into_prefix` and calls it on the way into `raw::Map::prefix`, which
//! clears the terminator for unsized readers and is a no-op for fixed-width
//! ones. Released as 0.1.8.
//!
//! Every test below fails on 0.1.7 or earlier and passes from 0.1.8. That is
//! the point of keeping them: the two call forms name the same byte sequence,
//! so they must select the same subtree, and if a validated key were not a
//! valid argument to `prefix` the type system would have to say so, because
//! an empty result is indistinguishable from a genuinely empty subtree.

use arctic::Order;
use arctic::key::BoxedStr;
use arctic::key::NonNull;
use arctic::key::Str;
use arctic::sequential;

const KEYS: [&str; 4] = [
    "fn:dot/loop:0",
    "fn:dot/loop:1",
    "fn:dot/loop:2",
    "fn:other/loop:0",
];

type Map = sequential::Map<BoxedStr<NonNull>, u64>;

fn loaded() -> Map {
    let mut map = Map::new();
    for (index, key) in KEYS.iter().enumerate() {
        let validated = Str::<NonNull>::new(*key).expect("No null byte");
        let _ = map.upsert(validated, index as u64);
    }
    map
}

/// `map.prefix("fn:dot".into())` - the call form that was always correct.
fn count_bare(map: &Map, prefix: &str) -> usize {
    map.prefix(prefix.into()).values(Order::Ascend).count()
}

/// `map.prefix(Str::new("fn:dot").unwrap().into())` - the call form that
/// compiled and lied.
fn count_validated(map: &Map, prefix: &str) -> usize {
    let validated = Str::<NonNull>::new(prefix).expect("No null byte");
    map.prefix(validated.into()).values(Order::Ascend).count()
}

/// Control. The bare `&str` call form is correct at every prefix length.
#[test]
fn bare_str_prefix_finds_the_subtree() {
    let map = loaded();
    assert_eq!(count_bare(&map, ""), 4);
    assert_eq!(count_bare(&map, "fn:"), 4);
    assert_eq!(count_bare(&map, "fn:dot"), 3);
    assert_eq!(count_bare(&map, "fn:dot/"), 3);
    assert_eq!(count_bare(&map, "fn:other"), 1);
    assert_eq!(count_bare(&map, "fn:dot/loop:0"), 1);
    assert_eq!(count_bare(&map, "fn:nothing"), 0);
}

/// The defect itself. Fails on 0.1.7, passes from 0.1.8.
#[test]
fn validated_key_prefix_must_match_bare_str() {
    let map = loaded();
    for prefix in ["", "fn:", "fn:dot", "fn:dot/", "fn:other", "fn:dot/loop:0"] {
        assert_eq!(
            count_validated(&map, prefix),
            count_bare(&map, prefix),
            "prefix({prefix:?}) disagrees between the validated and the bare call form; \
             a validated reader that keeps its terminator can only match a stored key \
             exactly, which turns prefix into get"
        );
    }
}

/// The narrowest statement of the harm, independent of the two call forms:
/// `prefix` under a parent must find everything `get` finds under it.
///
/// Fails on 0.1.7, passes from 0.1.8.
#[test]
fn prefix_must_find_every_key_that_get_finds_under_a_parent() {
    let map = loaded();
    let parent = Str::<NonNull>::new("fn:dot/").expect("No null byte");

    let mut found = 0;
    for key in KEYS {
        if key.starts_with("fn:dot/")
            && map
                .get(Str::<NonNull>::new(key).expect("No null byte"))
                .is_some()
        {
            found += 1;
        }
    }
    assert_eq!(found, 3, "get finds all three children");

    assert_eq!(
        map.prefix(parent.into()).values(Order::Ascend).count(),
        found,
        "prefix under the same parent must find what get finds"
    );
}

/// A prefix scan must not be confusable with an empty one. Every proper
/// prefix of a stored key selects at least that key, both call forms.
///
/// Fails on 0.1.7, passes from 0.1.8.
#[test]
fn every_proper_prefix_of_a_stored_key_selects_it() {
    let map = loaded();
    for key in KEYS {
        for end in 0..=key.len() {
            let Some(prefix) = key.get(..end) else {
                continue;
            };
            assert!(
                count_bare(&map, prefix) >= 1,
                "bare prefix({prefix:?}) must select {key:?}"
            );
            assert!(
                count_validated(&map, prefix) >= 1,
                "validated prefix({prefix:?}) must select {key:?}"
            );
        }
    }
}

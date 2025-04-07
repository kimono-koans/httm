//       ___           ___           ___           ___
//      /\__\         /\  \         /\  \         /\__\
//     /:/  /         \:\  \        \:\  \       /::|  |
//    /:/__/           \:\  \        \:\  \     /:|:|  |
//   /::\  \ ___       /::\  \       /::\  \   /:/|:|__|__
//  /:/\:\  /\__\     /:/\:\__\     /:/\:\__\ /:/ |::::\__\
//  \/__\:\/:/  /    /:/  \/__/    /:/  \/__/ \/__/~~/:/  /
//       \::/  /    /:/  /        /:/  /            /:/  /
//       /:/  /     \/__/         \/__/            /:/  /
//      /:/  /                                    /:/  /
//      \/__/                                     \/__/
//
// Copyright (c) 2023, Robert Swinford <robert.swinford<...at...>gmail.com>
//
// For the full copyright and license information, please view the LICENSE file
// that was distributed with this source code.

// this module is a re-implementation of the into_group_map() and into_group_map_by()
// methods for Iterator by Rust itertools team, for the purpose of using the same
// hashbrown hashmap used elsewhere in httm.  this was/is done for both performance
// and binary size reasons.
//
// see original: https://github.com/rust-itertools/itertools/blob/cfb2774fb02f61798967e89e1372bb95e625b7e6/src/group_map.rs#L25
//
// though I am fairly certain this re-implementation of their API is fair use
// I've reproduced their license, as of 11/25/2022, verbatim below:

// Copyright (c) 2015
//
// Permission is hereby granted, free of charge, to any
// person obtaining a copy of this software and associated
// documentation files (the "Software"), to deal in the
// Software without restriction, including without
// limitation the rights to use, copy, modify, merge,
// publish, distribute, sublicense, and/or sell copies of
// the Software, and to permit persons to whom the Software
// is furnished to do so, subject to the following
// conditions:
//
// The above copyright notice and this permission notice
// shall be included in all copies or substantial portions
// of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
// ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
// TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
// PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
// SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
// CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
// OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
// IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

use hashbrown::HashMap;
use hashbrown::HashSet;
use std::hash::Hash;
use std::iter::Iterator;

pub trait HttmIter: Iterator {
    #[allow(dead_code)]
    fn into_group_map<K, V>(self) -> HashMap<K, Vec<V>>
    where
        Self: Iterator<Item = (K, V)> + Sized,
        K: Hash + Eq,
    {
        group_map::into_group_map(self)
    }

    fn into_group_map_by<K, V, F>(self, f: F) -> HashMap<K, Vec<V>>
    where
        Self: Iterator<Item = V> + Sized,
        K: Hash + Eq,
        F: Fn(&V) -> K,
    {
        group_map::into_group_map_by(self, f)
    }

    #[allow(dead_code)]
    fn collect_map_no_update_values<K, V>(self) -> HashMap<K, V>
    where
        Self: Iterator<Item = (K, V)> + Sized,
        K: Hash + Eq,
    {
        collect_no_update::collect_map_no_update(self)
    }

    #[allow(dead_code)]
    fn collect_set_no_update_values<K>(self) -> HashSet<K>
    where
        Self: Iterator<Item = K> + Sized,
        K: Hash + Eq,
    {
        collect_no_update::collect_set_no_update(self)
    }
}

impl<T: ?Sized> HttmIter for T where T: Iterator {}

pub mod group_map {
    use hashbrown::HashMap;
    use std::hash::Hash;
    use std::iter::Iterator;

    pub fn into_group_map<I, K, V>(iter: I) -> HashMap<K, Vec<V>>
    where
        I: Iterator<Item = (K, V)>,
        K: Hash + Eq,
    {
        let mut lookup: HashMap<K, Vec<V>> = HashMap::with_capacity(iter.size_hint().0);

        iter.for_each(|(key, val)| match lookup.get_mut(&key) {
            Some(vec_val) => {
                vec_val.push(val);
            }
            None => {
                unsafe {
                    lookup.insert_unique_unchecked(key, [val].into());
                };
            }
        });

        lookup
    }

    pub fn into_group_map_by<I, K, V>(iter: I, f: impl Fn(&V) -> K) -> HashMap<K, Vec<V>>
    where
        I: Iterator<Item = V>,
        K: Hash + Eq,
    {
        into_group_map(iter.map(|v| (f(&v), v)))
    }
}

pub mod collect_no_update {
    use hashbrown::HashMap;
    use hashbrown::HashSet;
    use std::hash::Hash;
    use std::iter::Iterator;

    #[allow(dead_code)]
    pub fn collect_map_no_update<I, K, V>(iter: I) -> HashMap<K, V>
    where
        I: Iterator<Item = (K, V)>,
        K: Hash + Eq,
    {
        let mut lookup: HashMap<K, V> = HashMap::with_capacity(iter.size_hint().0);

        iter.for_each(|(key, val)| {
            if !lookup.contains_key(&key) {
                unsafe {
                    lookup.insert_unique_unchecked(key, val);
                };
            }
        });

        lookup
    }

    #[allow(dead_code)]
    pub fn collect_set_no_update<I, K>(iter: I) -> HashSet<K>
    where
        I: Iterator<Item = K>,
        K: Hash + Eq,
    {
        let mut lookup: HashSet<K> = HashSet::with_capacity(iter.size_hint().0);

        iter.for_each(|key| {
            if !lookup.contains(&key) {
                unsafe {
                    lookup.insert_unique_unchecked(key);
                };
            }
        });

        lookup
    }
}

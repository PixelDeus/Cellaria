//! Быстрый (не DoS-стойкий) хешер для внутренних hot-path структур движка.
//!
//! `std::collections::HashMap`/`HashSet` по умолчанию используют SipHash —
//! он защищён от HashDoS-атак через специально подобранные ключи, но платит
//! за это заметными накладными расходами на каждый хеш, особенно на мелких
//! ключах вроде `(usize, usize)`/`(u32, u32)`/`CellType(u8)`. Внутри движка
//! эти ключи — координаты клеток решётки и типы ячеек, вычисляемые из
//! состояния симуляции, а не из недоверенного внешнего ввода, так что
//! устойчивость к преднамеренным коллизиям тут не нужна, а скорость — нужна:
//! профиль показал, что на разреженных сценариях (одна активная клетка за
//! тик — движение головки машины Тьюринга, маркера сортировки) хеширование
//! в SipHash-структурах на пару тактов дороже самой полезной работы.
//!
//! Алгоритм — FxHash (используется внутри самого rustc для той же цели):
//! `hash = (hash.rotate_left(5) ^ word) * SEED`, применяется словами по 8/4/2/1
//! байт. Качество хеша ниже криптографического, но для наших ключей (малые,
//! неоднородные, не подбираются злоумышленником) коллизии не растут заметно
//! против SipHash — а лишней аллокации/раунда сжатия просто нет.

use std::hash::{BuildHasherDefault, Hasher};

const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

#[derive(Default)]
pub(crate) struct FxHasher {
    hash: u64,
}

impl FxHasher {
    #[inline]
    fn add_to_hash(&mut self, word: u64) {
        self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(SEED);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, mut bytes: &[u8]) {
        while bytes.len() >= 8 {
            self.add_to_hash(u64::from_ne_bytes(
                bytes[..8]
                    .try_into()
                    .expect("slice was just checked to have length >= 8"),
            ));
            bytes = &bytes[8..];
        }
        if bytes.len() >= 4 {
            self.add_to_hash(u32::from_ne_bytes(
                bytes[..4]
                    .try_into()
                    .expect("slice was just checked to have length >= 4"),
            ) as u64);
            bytes = &bytes[4..];
        }
        if bytes.len() >= 2 {
            self.add_to_hash(u16::from_ne_bytes(
                bytes[..2]
                    .try_into()
                    .expect("slice was just checked to have length >= 2"),
            ) as u64);
            bytes = &bytes[2..];
        }
        if let Some(&b) = bytes.first() {
            self.add_to_hash(b as u64);
        }
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add_to_hash(i as u64);
    }
    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.add_to_hash(i as u64);
    }
    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add_to_hash(i as u64);
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add_to_hash(i);
    }
    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add_to_hash(i as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }
}

pub(crate) type FxBuildHasher = BuildHasherDefault<FxHasher>;
pub(crate) type FxHashMap<K, V> = std::collections::HashMap<K, V, FxBuildHasher>;
pub(crate) type FxHashSet<K> = std::collections::HashSet<K, FxBuildHasher>;

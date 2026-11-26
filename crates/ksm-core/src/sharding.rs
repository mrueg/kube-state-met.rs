//! Port of `pkg/sharding`: an object belongs to shard
//! `jump(fnv64a(uid), total_shards)`, exactly as in Go (ADR-005).

/// FNV-1a, 64 bit (`hash/fnv.New64a`).
pub fn fnv64a(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Jump consistent hash (`github.com/dgryski/go-jump.Hash`).
pub fn jump_hash(mut key: u64, num_buckets: i64) -> i32 {
    let mut b: i64 = -1;
    let mut j: i64 = 0;
    while j < num_buckets {
        b = j;
        key = key.wrapping_mul(2_862_933_555_777_941_757).wrapping_add(1);
        #[expect(
            clippy::cast_precision_loss,
            reason = "mirrors go-jump's float arithmetic"
        )]
        let next = ((b + 1) as f64) * ((1i64 << 31) as f64 / ((key >> 33) + 1) as f64);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "mirrors go-jump's int64 conversion"
        )]
        {
            j = next as i64;
        }
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "bucket < num_buckets fits int32 as in Go"
    )]
    {
        b as i32
    }
}

/// Shard settings of this instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sharding {
    pub shard: i32,
    pub total_shards: i64,
}

impl Sharding {
    pub fn new(shard: i32, total_shards: i64) -> Self {
        Sharding {
            shard,
            total_shards,
        }
    }

    /// No sharding: every object is kept.
    pub fn is_disabled(&self) -> bool {
        self.shard == 0 && self.total_shards == 1
    }

    /// Whether this shard keeps the object with `uid`.
    pub fn keep(&self, uid: &str) -> bool {
        self.is_disabled() || jump_hash(fnv64a(uid.as_bytes()), self.total_shards) == self.shard
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_vectors() {
        assert_eq!(fnv64a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv64a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn jump_vectors() {
        // From go-jump's jump_test.go (TestHash, TestGolden).
        let table: &[(u64, &[i32])] = &[
            (
                1,
                &[0, 0, 0, 0, 0, 0, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 17, 17],
            ),
            (
                0xdead_beef,
                &[0, 1, 2, 3, 3, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 16, 16, 16],
            ),
            (
                0x0ddc_0ffe_ebad_f00d,
                &[0, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 15, 15, 15, 15],
            ),
        ];
        for (key, buckets) in table {
            for (i, want) in buckets.iter().enumerate() {
                assert_eq!(
                    jump_hash(*key, i as i64 + 1),
                    *want,
                    "jump({key}, {})",
                    i + 1
                );
            }
        }
        let golden100 = [
            0, 55, 62, 8, 45, 59, 86, 97, 82, 59, 73, 37, 17, 56, 86, 21, 90, 37, 38, 83,
        ];
        for (i, want) in golden100.iter().enumerate() {
            assert_eq!(jump_hash(i as u64, 100), *want);
        }
        assert_eq!(jump_hash(10_863_919_174_838_991, 11), 6);
        assert_eq!(jump_hash(2_016_238_256_797_177_309, 11), 3);
        assert_eq!(jump_hash(1_673_758_223_894_951_030, 11), 5);
        assert_eq!(jump_hash(2, 100_001), 80_343);
    }
}

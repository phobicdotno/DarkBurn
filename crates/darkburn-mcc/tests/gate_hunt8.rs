//! Bug hunt pass 8: the gate in front of the card. Each test here failed on the code before its fix.

use darkburn_mcc::registers::BLOCKS;

#[test]
fn a_block_does_not_claim_a_range_that_wraps_past_the_last_register() {
    // `addr + count` wrapped (or stopped the program in a debug build): the range that starts at
    // the last register and is two long "ended" at register 1, inside every block.
    for b in BLOCKS {
        assert!(!b.contains(u32::MAX, 2), "{} claims a wrapped range", b.name);
        assert!(!b.contains(b.start, u32::MAX), "{} claims a huge range", b.name);
        assert!(b.contains(b.start, b.count));
    }
}

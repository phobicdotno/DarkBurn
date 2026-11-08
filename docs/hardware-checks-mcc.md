# Hardware checks and open questions: Gweike MCC100 (`darkburn-mcc`)

These are the facts `crates/darkburn-mcc` relies on that nobody has confirmed on the real card, plus
decisions that belong to the owner. Nothing in the crate talks to a machine. Per `docs/BUILD-RULES.md`,
no Gweike motion code runs until a **read-only capture** on the real card has closed the gaps in the
owner's `99-gaps.md`. MCC-06 (native Ethernet backend) and TARGET-03 stay owner-only.

Legend: **observed** = seen in a vendor log or shipped file by the owner's analysis; **inferred** = the
owner's reading of static analysis; **simulator choice** = a value the simulator invents so tests can run.

## 1. Decisions for the owner

| # | Question | Where it matters |
|---|---|---|
| D1 | The owner's analysis allows `0x65 <- [101]` (follower stop) in the stop sequence. The project rule says never write `0x65 <- [0x65]`. The crate follows the rule: `[101]` and `[102]` are unrepresentable, and the stop sequence has no follower stop. Is that what you want, or should the rule be narrowed to `[102]` only after a capture shows what `[101]` does? | `command.rs`, `stop_sequence` |
| D2 | `encoding_rs` is `(Apache-2.0 OR MIT) AND BSD-3-Clause`. The crate keeps Chinese `.chf` text behind the off-by-default `cp936` feature. Accept the extra BSD-3-Clause notice (then make it default), or keep text ASCII-only and raw bytes? | `Cargo.toml`, `chf.rs` |
| D3 | Versions 2 to 4 of `.chf` carry reserved blank lines. The crate reads and writes them blank; a file with non-blank reserved lines will not round-trip byte for byte. The owner's Python keeps them. Worth porting that? (No shipped sample needs it.) | `chf.rs` |
| D4 | Laser-enabling writes (DO5 / DO6 / DO9 on, PWM with duty, non-zero DA) are classified by the gate model but cannot be encoded by the public API. Keep it that way until the first real cut, then add them behind an explicit, separately reviewed API? | `command.rs`, `gate.rs` |

## 2. To confirm with a read-only capture (from `99-gaps.md` and `11-static-findings.md` section 7)

| # | Unconfirmed fact | Used by | Status |
|---|---|---|---|
| C1 | Success reply layouts (READ reply `addr, count, words`; WRITE echo `addr, count`). Only exception replies were ever logged. | `Reply::encode` / `decode`, simulator | derived from the PC decoder |
| C2 | Register 1016 unit: bytes free, 60000 = empty. | `FifoFeeder`, `queued_bytes`, simulator | inferred |
| C3 | FIFO tick period: register 50005 default 250 us. | `PlanConfig::tick_us`, simulator, time estimate | inferred (XML default) |
| C4 | Pulses per mm: X `8000/31.003`, Y `8000/31.009`. The vendor takes the X factor from the card (50200 block, words 9 and 10) and the Y factor from XML. | `PlanConfig` | inferred (config file) |
| C5 | Frame id rule: register 1015 plus one, repeated id acknowledged and not queued. | `FifoFeeder`, simulator | inferred / observed |
| C6 | Frame flush at 300 words including the 4-word prefix (99 ticks). | `FramePacker` | inferred |
| C7 | Stop vectors: `[1,mask,2,vd,10vd]` and `0x67 <- [3]`. Meaning of the "2" word and of `vd` (deceleration in mm/s^2). | `stop_all`, `jog_release_stop` | observed vectors, inferred meaning |
| C8 | Jog distances are relative; bit 31 of the axis word means absolute. | `jog_*`, `move_axis_absolute`, `go_to` | inferred, medium-high |
| C9 | Axis command types 3, 4, 5 (jog / move variants), homed bit 15, busy byte 2. | `registers::axis`, simulator | inferred |
| C10 | Alarm 1 bit 24 meaning (follower axis) and the "alone is not an alarm while idle" rule. | `gate.rs` | inferred, low-medium |
| C11 | Whether the end of a clean job raises alarm 2 bit 5 (FIFO starvation). The simulator raises it by default; the gate then disarms. | `SimConfig::fifo_starvation_alarm` | open |
| C12 | Connect write `[9999, 5, 0, 0]`: needed or not, meaning unknown. | `connect_prologue` | observed, meaning unknown |
| C13 | Register `0x64 <- 8888` written at connect by Mlaser 0.0.1.51 SP1 (log `2026-09-16`). Meaning unknown; this crate does **not** implement it, and register 100 is on the deny list. | `registers.rs` | open |
| C14 | Resume after pause: no primitive known. | n/a (not implemented) | open |
| C15 | Pause equals stop: no resume path is modelled. | `gate.rs` | open |
| C16 | `MinHardwareVer`: 20152 in 0.0.0.52, 20177 in 0.0.1.51 SP1 (firmware V201.77). The simulator reports 20152. | `SimConfig::program_version` | observed, both values |
| C17 | Whether register 1004 is the raw input level or already inverted by the card's own input-type register 5000. | `StatusBlock::di_level` | open |
| C18 | Whether the card accepts a request with a CRC in the other byte order. The simulator only accepts the PC order and drops anything else. | `frame.rs`, simulator | simulator choice |
| C19 | Gas pressure unit: values behave like bar through a calibration table; the vendor descriptor says volts. | `craft.rs` | inferred, high |
| C20 | Gas codes 0 to 5 and process type codes 0 to 4; codes above 4 for `ManuType` are not established. | `craft.rs` | inferred |
| C21 | Soft limits X 1371 mm, Y 950 mm, lower bound 0; K = 1000 (register 50017). | `SafetyConfig::soft_limits_word` | inferred (config file) |

## 3. Simulator choices (not card behaviour)

* Exception codes for refusals: 2 for unmapped reads, forbidden requests and laser data; 3 for a motion command
  while busy and for a full FIFO (the owner's logs show 502 and 503 but not their causes).
* Motion takes 300 ms; a stop cancels a motion and leaves the axis at its start position.
* FIFO capacity is 60000 bytes; a frame that does not fit gets exception 3.
* Position in block 2000 is reported in card units (micrometres), the real card reports pulses.
* Reboot: silent window, then an exception 2 window.

## 4. Planner estimate

The time estimate is the tick count times the tick period, from a rest-to-rest trapezoid per segment. The
vendor planner has look-ahead and corner blending, so on chains of short segments the estimate is an upper
bound. Once C2 and C3 are known, compare the estimate with the real duration of a dry move (capture step 8 in
`11-static-findings.md`).

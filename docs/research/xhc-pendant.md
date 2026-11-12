# XHC wireless handheld pendant (PHB02)

Date: 2026-11-12. The protocol below is as observed on the device: the owner's PHB02 pendant and
its USB receiver, watched through the receiver's HID reports while keys were pressed and LED
reports were sent. The `darkburn-pendant` crate implements it; `apps/darkburn/src/pendant_ui.rs`
wires it into the app.

## Receiver

- USB vendor 0x10ce, product 0xeba0, a HID device with a vendor usage page (0xff00).
- Other XHC receivers seem to use products in the range 0xeb61..0xeba1. Only 0xeba0 has been
  seen, so only it is in `PRODUCT_IDS`.
- On Linux it is a `/dev/hidraw*` node that only root can open. `packaging/linux/70-darkburn-pendant.rules`
  (installed by `packaging/linux/install-pendant-rule.sh`, or by the command the app shows) gives
  the logged-in user access.

## Keys: input report 4

Six bytes: `04 10 k1 k2 k3 chk`.

- `k1..k3` are the codes of the keys held right now, zero for none. Each report is the full set
  of held keys, so presses and releases come from comparing one report with the last.
- Byte 1 (always 0x10 so far) and `chk` are not checked.
- With nothing held, an all-zero report arrives about once a second.

| Code | Key | Code | Key |
|---|---|---|---|
| 0x01 | Follow | 0x0b | Fn |
| 0x02 | Shutter | 0x0c | Z calibrate |
| 0x03 | Red light | 0x0d | Fast step |
| 0x04 | Laser | 0x0e | Z- (W) |
| 0x05 | Blow | 0x0f | Up |
| 0x06 | Frame | 0x10 | Z+ (W) |
| 0x07 | Go to origin | 0x11 | Left |
| 0x08 | Start | 0x12 | Down |
| 0x09 | Pause | 0x13 | Right |
| 0x0a | Stop | | |

Fn is an ordinary key code; Fn+key is simply both codes in one report. The pendant prints Fn
functions on Go to origin, Z calibrate, Z- and Z+.

## LEDs: feature report 6

Eight bytes: `06 fe fd SEQ LEDS 00 00 00`. `SEQ` counts up by one per message.

| Bit | LED |
|---|---|
| 0x01 | Laser |
| 0x02 | Blow |
| 0x04 | Frame |
| 0x10 | Follow |
| 0x20 | Shutter |
| 0x40 | Red light |

The receiver queues LED messages and the pendant picks them up one at a time when it wakes, so
a burst of messages plays out slowly. DarkBurn sends one only when the wanted state changes, at
most one per 250 ms.

## In DarkBurn

- Off by default ("Use wireless pendant" in Settings). Shared defaults and imported settings
  never turn it on. Never in safe mode.
- Every key runs the command its on-screen button runs, through the same path, and only when
  that button would be enabled. The Laser key does nothing.
- Map, LEDs and the keys still to do: see the header of `apps/darkburn/src/pendant_ui.rs`.
